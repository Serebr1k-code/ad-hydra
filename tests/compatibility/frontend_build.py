"""Isolated bundle freshness checks; never touches the actual frontend dist."""
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]


class FrontendBuild(unittest.TestCase):
    def test_root_configuration_edits_removals_and_renames(self):
        with tempfile.TemporaryDirectory(prefix='hydra-build-script-') as directory:
            executable = Path(directory) / 'frontend-build'
            subprocess.run(['rustc', '--edition', '2024', str(ROOT / 'apps/hydra-core/build.rs'), '-o', str(executable)], check=True)
            for name in ['.env', '.env.local', '.env.production', '.env.production.local', 'vite.config.ts']:
                for operation in ['edit', 'delete', 'rename']:
                    with self.subTest(name=name, operation=operation), tempfile.TemporaryDirectory(prefix='hydra-frontend-fixture-') as fixture:
                        root = Path(fixture)
                        app = root / 'apps/hydra-core'
                        app.mkdir(parents=True)
                        frontend = root / 'frontend'
                        (frontend / 'src').mkdir(parents=True)
                        output = root / 'out'
                        output.mkdir()
                        for item in {'index.html', 'package.json', 'bun.lock', 'src/main.ts', 'vite.config.ts', 'tsconfig.json', name}:
                            (frontend / item).write_text('input')
                        (frontend / 'dist/assets').mkdir(parents=True)
                        (frontend / 'dist/index.html').write_text('index')
                        (frontend / 'dist/assets/index-abcd1234.js').write_text('compiled')
                        old = time.time() - 30
                        for path in frontend.rglob('*'):
                            os.utime(path, (old, old))
                        for path in (frontend / 'dist').rglob('*'):
                            if path.is_file():
                                os.utime(path, (old + 10, old + 10))
                        os.utime(frontend, (old, old))
                        environment = dict(os.environ, CARGO_MANIFEST_DIR=str(app), OUT_DIR=str(output))
                        def build():
                            return subprocess.run([str(executable)], env=environment, capture_output=True, text=True)
                        baseline = build()
                        self.assertEqual(baseline.returncode, 0, baseline.stderr)
                        path = frontend / name
                        if operation == 'edit':
                            path.write_text('changed')
                        elif operation == 'delete':
                            path.unlink()
                        else:
                            path.rename(frontend / (name + '.renamed'))
                        stale = build()
                        self.assertNotEqual(stale.returncode, 0)
                        self.assertIn('Stale frontend/dist', stale.stderr)


if __name__ == '__main__':
    unittest.main()
