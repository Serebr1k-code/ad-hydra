"""Verify CPU-aware shared runtime pools with many real loopback TCP sessions."""
import asyncio
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
from relay import BIN, port


async def check(cpus):
    async def echo(reader, writer):
        try:
            while data := await reader.read(16384):
                writer.write(data)
                await writer.drain()
        finally:
            writer.close()
            await writer.wait_closed()

    server = await asyncio.start_server(echo, '127.0.0.1', 0)
    async with server:
        with tempfile.TemporaryDirectory(prefix='hydra-runtime-') as directory:
            root = Path(directory)
            address = port()
            config = dict(socket=str(root/'proxy.sock'), state_file=str(root/'state'), snapshot=dict(revision=1,
                          services=[dict(id='runtime', transport='tcp', listen_address=f'127.0.0.1:{address}',
                          upstream_address=f'127.0.0.1:{server.sockets[0].getsockname()[1]}',
                          protocol_id='raw', enabled=True, retain_history=False)]))
            (root/'config').write_text(json.dumps(config))
            with (root/'log').open('w+') as log:
                proxy = subprocess.Popen(['taskset', '-c', ','.join(map(str, cpus)), str(BIN/'hydra-proxy'),
                                          '--config', str(root/'config')], stdout=log, stderr=log)
                peers = []
                try:
                    for _ in range(300):
                        try:
                            peers.append(await asyncio.open_connection('127.0.0.1', address))
                            break
                        except OSError:
                            if proxy.poll() is not None: raise AssertionError((root/'log').read_text())
                            await asyncio.sleep(.01)
                    else: raise AssertionError('Proxy did not start')
                    startup = [json.loads(line) for line in (root/'log').read_text().splitlines()]
                    runtime = next((row['fields'] for row in startup if row['fields'].get('message') == 'proxy runtime configured'), None)
                    assert runtime is not None, 'Proxy must report its actual shared runtime worker count'
                    workers = runtime['async_workers']
                    assert 1 <= workers <= len(cpus), runtime
                    if len(cpus) == 1: assert workers == 1, runtime
                    assert runtime['blocking_workers_limit'] == min(workers, 4), runtime
                    # 128 live sessions must share a CPU-sized thread pool.
                    peers += await asyncio.gather(*(asyncio.open_connection('127.0.0.1', address) for _ in range(127)))
                    async def exchange(peer):
                        reader, writer = peer
                        payload = bytes(range(256))*16
                        for _ in range(64):
                            writer.write(payload)
                            await writer.drain()
                            assert await asyncio.wait_for(reader.readexactly(len(payload)), 5) == payload
                    started = time.monotonic()
                    await asyncio.gather(*(exchange(peer) for peer in peers))
                    threads = len(list(Path(f'/proc/{proxy.pid}/task').iterdir()))
                    # Main + async pool + capped blocking pool + critical spool.
                    assert threads <= workers + min(workers, 4) + 2, (workers, threads)
                    print(json.dumps(dict(affinity=cpus, async_workers=workers, os_threads=threads,
                                          concurrent_sessions=len(peers), verified_exchanges=len(peers)*64,
                                          elapsed_seconds=time.monotonic()-started)))
                finally:
                    for _, writer in peers: writer.close()
                    await asyncio.gather(*(writer.wait_closed() for _, writer in peers), return_exceptions=True)
                    proxy.terminate(); proxy.wait(timeout=10)


async def main():
    cpus = sorted(os.sched_getaffinity(0))
    await check(cpus[:1])
    if len(cpus) > 1: await check(cpus[:min(3, len(cpus))])


if __name__ == '__main__':
    asyncio.run(main())
