"""从 Cargo 构建消息获取产物位置，遵循工具链和目标目录配置。"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def build(*arguments, cwd=ROOT, env=None):
    environment = dict(os.environ if env is None else env)
    cargo = environment.get('CARGO') or shutil.which('cargo')
    if not cargo:
        cargo = str(Path(environment.get('CARGO_HOME', Path.home() / '.cargo')) / 'bin/cargo')
    environment['PATH'] = str(Path(cargo).resolve().parent) + os.pathsep + environment.get('PATH', '')
    result = subprocess.run([cargo, 'build', '--message-format=json-render-diagnostics', *arguments],
                            cwd=cwd, env=environment, text=True, stdout=subprocess.PIPE, check=True)
    artifacts = {}
    for line in result.stdout.splitlines():
        message = json.loads(line)
        if message.get('reason') == 'compiler-artifact' and (
                message.get('executable') or 'staticlib' in message['target']['crate_types']):
            artifacts[message['target']['name']] = {
                'executable': message.get('executable'), 'filenames': message['filenames'],
            }
    return artifacts


def host_binary():
    return os.environ.get('ANYWHERE_TEST_BINARY') or build(
        '--locked', '-p', 'anywhere-host', '--bins')['anywhere-host']['executable']


if __name__ == '__main__':
    print(json.dumps(build(*sys.argv[1:])))
