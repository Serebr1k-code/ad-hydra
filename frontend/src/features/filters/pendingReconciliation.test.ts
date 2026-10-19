import { expect, test } from 'bun:test'
import { createPendingReconciliation } from './pendingReconciliation'

function fixture() {
  let pending = true, paused = false, reads = 0, fail = false
  const accepted: number[] = [], errors: unknown[] = []
  let queued: { run: () => Promise<void>; delay: number } | undefined
  let resolve: ((value: number) => void) | undefined
  const reconciler = createPendingReconciliation({
    pending: () => pending, paused: () => paused,
    read: async () => { reads++; if (fail) throw new Error('offline'); return new Promise<number>(done => { resolve = done }) },
    accept: value => { accepted.push(value); pending = value === 0 },
    failed: cause => errors.push(cause),
    schedule: (run, delay) => { const entry = { run: async () => { if (queued === entry) queued = undefined; await run() }, delay }; queued = entry; return () => { if (queued === entry) queued = undefined } },
  })
  return { reconciler, accepted, errors, reads: () => reads, queue: () => queued, finish: (value: number) => resolve!(value), setPaused: (value: boolean) => { paused = value }, setFail: () => { fail = true } }
}

test('pending application retries quickly, backs off and stops after an ACK', async () => {
  const f = fixture(); f.reconciler.resume()
  for (let i = 0; i < 4; i++) { const task = f.queue()!.run(); f.finish(0); await task }
  expect(f.queue()!.delay).toBe(5000)
  const task = f.queue()!.run(); f.finish(9); await task
  expect(f.accepted).toEqual([0, 0, 0, 0, 9])
  expect(f.queue()).toBeUndefined()
})

test('invalidating a poll prevents overwriting newer mutations and prevents overlapping reads', async () => {
  const f = fixture(); f.reconciler.resume()
  const stale = f.queue()!.run()
  f.reconciler.invalidate(); f.reconciler.resume()
  await f.queue()!.run()
  expect(f.reads()).toBe(1)
  f.finish(9); await stale
  expect(f.accepted).toEqual([])
  const fresh = f.queue()!.run(); f.finish(9); await fresh
  expect(f.accepted).toEqual([9])
})

test('hidden or busy pages pause reads and disposal ignores in-flight responses', async () => {
  const f = fixture(); f.setPaused(true); f.reconciler.resume(); await f.queue()!.run()
  expect(f.reads()).toBe(0)
  expect(f.queue()!.delay).toBe(5000)
  f.setPaused(false)
  const task = f.queue()!.run(); f.reconciler.stop(); f.finish(9); await task
  expect(f.accepted).toEqual([])
  expect(f.queue()).toBeUndefined()
  f.reconciler.resume()
  expect(f.queue()).toBeUndefined()
})

test('read failures surface without accepting data and retry only at the slow interval', async () => {
  const f = fixture(); f.setFail(); f.reconciler.resume(); await f.queue()!.run()
  expect(f.accepted).toEqual([])
  expect(f.errors).toHaveLength(1)
  expect(f.queue()!.delay).toBe(5000)
})
