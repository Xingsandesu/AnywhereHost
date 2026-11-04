#!/usr/bin/env python3
"""检查提交身份与消息；错误信息不回显邮箱。"""
import argparse
from pathlib import Path
import re
import subprocess
import sys


PUBLIC_EMAIL = re.compile(
    rb'(?:[A-Za-z0-9+\[\]-]+@users\.noreply\.github\.com|noreply@(?:github\.com|openai\.com))'
)
EMAIL = re.compile(rb'[A-Za-z0-9.!#$%&\x27*+/=?^_`{|}~-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}')


def git(*args):
    return subprocess.check_output(['git', *args], stderr=subprocess.PIPE)


def check_identity(value, label):
    match = re.search(rb'<([^<>]+)> \d+ [+-]\d{4}$', value.strip())
    if not match or not PUBLIC_EMAIL.fullmatch(match[1]):
        raise ValueError(f'{label}: use a GitHub noreply identity')


def check_object(oid):
    kind = git('cat-file', '-t', oid).strip()
    if kind not in (b'commit', b'tag'):
        return
    data = git('cat-file', kind.decode(), oid)
    header, _, message = data.partition(b'\n\n')
    for line in header.splitlines():
        if line.startswith((b'author ', b'committer ', b'tagger ')):
            check_identity(line, oid[:12])
    check_message(message, oid[:12])


def check_message(message, label):
    if any(not PUBLIC_EMAIL.fullmatch(email) for email in EMAIL.findall(message)):
        raise ValueError(f'{label}: non-public email in commit or tag message')


def check_history(refs):
    commits = git('rev-list', *refs).decode().splitlines()
    for oid in set(commits):
        check_object(oid)
    # Annotated tags carry a separate tagger identity and may point to other tags.
    tags = set(git('for-each-ref', '--format=%(objectname)', 'refs/tags').decode().splitlines())
    # A tag can also be pushed directly by object ID without a local tag ref.
    tags.update(ref for ref in refs if ref != '--all')
    for oid in tags:
        while git('cat-file', '-t', oid).strip() == b'tag':
            check_object(oid)
            oid = git('cat-file', 'tag', oid).splitlines()[0].split()[1].decode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--identity', action='store_true')
    mode.add_argument('--push', action='store_true')
    mode.add_argument('--message', type=Path)
    args = parser.parse_args()
    if args.identity:
        for key in ('GIT_AUTHOR_IDENT', 'GIT_COMMITTER_IDENT'):
            check_identity(git('var', key), key)
    elif args.message:
        check_message(args.message.read_bytes(), 'message')
    elif args.push:
        refs = [line.split()[1] for line in sys.stdin if line.strip()]
        refs = [oid for oid in refs if set(oid) != {'0'}]
        if refs:
            check_history(refs)
    else:
        check_history(['--all'])


if __name__ == '__main__':
    try:
        main()
    except (ValueError, subprocess.CalledProcessError) as error:
        detail = str(error) if isinstance(error, ValueError) else 'Git inspection failed'
        print(f'Git privacy check failed: {detail}', file=sys.stderr)
        sys.exit(1)
