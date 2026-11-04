#!/usr/bin/env python3
"""Deterministic protocol peer. Never reads real Codex files or contacts a model."""
import json, os, pathlib, sys
METHODS = ['initialize', 'project/list', 'project/read', 'thread/list', 'thread/start', 'thread/fork', 'thread/resume', 'thread/settings/update', 'thread/turns/list', 'thread/items/list', 'turn/start', 'turn/interrupt', 'model/list']
if 'generate-json-schema' in sys.argv:
    directory = pathlib.Path(sys.argv[sys.argv.index('--out') + 1])
    (directory / 'ClientRequest.json').write_text(json.dumps({'oneOf': [{'properties': {'method': {'enum': [m]}}} for m in METHODS]}))
    sys.exit(0)

def send(value):
    print(json.dumps(value), flush=True)

root = os.environ['CODEX_HOME']
project = {'id': 'project-1', 'name': 'Fixture project', 'roots': [{'path': root}]}
materialized = False
for line in sys.stdin:
    req = json.loads(line)
    if req.get('id') == 'approval-1' and 'result' in req:
        send({'method': 'item/started', 'params': {'threadId': 'owned-1', 'turnId': 'turn-1', 'item': {'id': 'item-1', 'type': 'agentMessage', 'text': ''}}})
        send({'method': 'item/agentMessage/delta', 'params': {'threadId': 'owned-1', 'turnId': 'turn-1', 'itemId': 'item-1', 'delta': 'Approved fixture'}})
        send({'method': 'item/completed', 'params': {'threadId': 'owned-1', 'turnId': 'turn-1', 'item': {'id': 'item-1', 'type': 'agentMessage', 'text': 'Approved fixture'}}})
        send({'method': 'turn/completed', 'params': {'threadId': 'owned-1', 'turn': {'id': 'turn-1', 'status': 'completed'}}})
        continue
    if 'id' not in req: continue
    method = req['method']; p = req.get('params', {})
    with (pathlib.Path(root) / 'requests.jsonl').open('a') as log:
        log.write(json.dumps({'method': method, 'params': p}) + '\n')
    if method == 'initialize': result = {'userAgent': 'fixture/1', 'codexHome': root}
    elif method == 'project/list': result = {'data': [project], 'nextCursor': None}
    elif method == 'project/read': result = {'project': project}
    elif method == 'thread/list':
        assert 'appServer' in p['sourceKinds']
        result = {'data': [{'id': 'desktop-1', 'name': 'Existing desktop thread', 'cwd': root}], 'nextCursor': None}
    elif method in ('thread/start', 'thread/fork'): result = {'thread': {'id': 'owned-1', 'cwd': p.get('cwd', root)}}
    elif method == 'thread/resume':
        assert p['threadId'] == 'owned-1'
        result = {'thread': {'id': 'owned-1'}, 'cwd': root, 'model': 'fixture'}
    elif method == 'thread/settings/update':
        for index in range(p.get('fixtureEvents', 0)):
            send({'method': 'fixture/replay', 'params': {'index': index}})
        if p.get('fixtureOversizedEvent'):
            send({'method': 'fixture/large', 'params': {'text': 'x' * (4 * 1024 * 1024)}})
        result = p
    elif method == 'turn/start':
        materialized = True
        send({'id': req['id'], 'result': {'turn': {'id': 'turn-1', 'status': 'inProgress'}}})
        send({'id': 'approval-1', 'method': 'item/commandExecution/requestApproval', 'params': {'threadId': 'owned-1', 'turnId': 'turn-1', 'command': 'fixture read only'}})
        continue
    elif method == 'thread/turns/list':
        if p['threadId'] == 'missing':
            send({'id': req['id'], 'error': {'code': -32600, 'message': 'thread not found'}})
            continue
        if p['threadId'] == 'owned-1' and not materialized:
            send({'id': req['id'], 'error': {'code': -32600, 'message': 'thread owned-1 is not materialized yet; thread/turns/list is unavailable before first user message'}})
            continue
        result = {'data': [{'id': 'turn-1', 'status': 'completed', 'items': []}] if materialized else [], 'nextCursor': None}
    elif method == 'thread/items/list':
        entries=[{'turnId':'turn-1','item':{'id':f'item-{i}','type':'commandExecution','command':f'command {i}','aggregatedOutput':'details','status':'completed'}} for i in range(240)]
        entries.reverse(); offset=int(p.get('cursor',0)); limit=p.get('limit',60)
        result={'data':entries[offset:offset+limit], 'nextCursor':str(offset+limit) if offset+limit<len(entries) else None}
    elif method == 'model/list': result = {'data': []}
    else:
        send({'id': req['id'], 'error': {'code': -32601, 'message': 'Unsupported fixture method'}})
        continue
    send({'id': req['id'], 'result': result})
