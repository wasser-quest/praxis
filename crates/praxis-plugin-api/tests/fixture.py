"""Local protocol fault fixture; never starts a VM or calls a provider."""
import json
import os
import pathlib
import struct
import sys

mode, log_path = sys.argv[1:3]
log = pathlib.Path(log_path)

def read():
    header = sys.stdin.buffer.read(4)
    if len(header) != 4:
        raise EOFError
    length = struct.unpack('>I', header)[0]
    return json.loads(sys.stdin.buffer.read(length))

def write(value):
    data = json.dumps(value).encode()
    sys.stdout.buffer.write(struct.pack('>I', len(data)) + data)
    sys.stdout.buffer.flush()

hello = read()
nonce = hello['nonce']
write(dict(type='ready', version=99 if mode == 'bad_version' else 1,
           owner=hello['owner'], service=hello['service'], nonce=nonce,
           operations=['echo'], controls=[]))
if mode == 'idle_crash':
    sys.exit(7)
while True:
    request = read()
    rid = request['id']
    kind = request['type']
    if kind == 'shutdown':
        write(dict(type='stopped', id=rid, nonce=nonce))
        break
    if kind == 'health':
        write(dict(type='completed', id=rid, nonce=nonce, result={'healthy': True}))
        continue
    with log.open('a') as output:
        output.write('invoke\n')
    if mode == 'crash':
        sys.exit(7)
    if mode == 'cancel':
        cancellation = read()
        assert cancellation['type'] == 'cancel' and cancellation['id'] == rid
        log.with_suffix('.cancelled').write_text('acknowledged')
        write(dict(type='cancelled', id=rid, nonce=nonce))
        break
    if mode == 'oversize':
        sys.stdout.buffer.write(struct.pack('>I', 2 * 1024 * 1024 + 1))
        sys.stdout.buffer.flush()
        continue
    if mode == 'failure':
        write(dict(type='failed', id=rid, nonce=nonce, code='PRIVATE_IMPLEMENTATION_SECRET'))
        continue
    result = {'input': request['input'], 'context': request['context'],
              'inherited_secret': os.environ.get('PRAXIS_TEST_IPC_SECRET')}
    write(dict(type='completed', id=rid + (1 if mode == 'wrong_id' else 0),
               nonce='wrong' if mode == 'wrong_nonce' else nonce, result=result))
