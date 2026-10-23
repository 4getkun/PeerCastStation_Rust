"""Standard-library review checks only. Not a Rust implementation or peer test."""
from pathlib import Path
import hashlib, json, re, struct, uuid, ipaddress
ROOT=Path(__file__).resolve().parent

def atom(name, value):
    name=name.encode('ascii').ljust(4,b'\0')
    assert len(name)==4
    return name+struct.pack('<I',len(value))+value

def dotnet_string(text):
    b=text.encode('utf-8'); n=len(b); prefix=bytearray()
    while n>=128:
        prefix.append((n&127)|128); n>>=7
    prefix.append(n)
    return bytes(prefix)+b

def station_id(bcid, network, name, genre, source):
    assert network in (0,1)
    pre=(struct.pack('<i',network) if network else b'')
    pre+=hashlib.sha512(uuid.UUID(bcid).bytes_le).digest()
    pre+=b''.join(map(dotnet_string,[name,genre,source]))
    digest=hashlib.md5(pre).digest()
    return uuid.UUID(bytes_le=digest).hex

def check():
    checks=0
    def ok(condition, label):
        nonlocal checks
        assert condition,label
        checks+=1
    manifest=json.loads((ROOT/'manifest.json').read_text())
    for name,h in manifest['input_sha256'].items():
        ok(hashlib.sha256((ROOT/'originals'/name).read_bytes()).hexdigest()==h,'original '+name)
    spec=(ROOT/'SPECIFICATION.md').read_text()
    ids=re.findall(r'^REQ-(\d+):',spec,re.M)
    ok(len(ids)==len(set(ids)), 'unique requirement definitions')
    ok(len(ids)>=45,'nonempty requirement inventory')
    ev=json.loads((ROOT/'source-evidence.json').read_text())
    ok(len(ev)==36,'evidence count')
    ok(len({e['id'] for e in ev})==len(ev),'unique evidence IDs')
    for e in ev:
        ok(bool(re.fullmatch(r'[0-9a-f]{40}',e['commit'])),'pinned commit '+e['id'])
        ok('/blob/'+e['commit']+'/' in e['url'] and e['url'].endswith('#L'+str(e['line'])),'source URL '+e['id'])
    for v in json.loads((ROOT/'reference_vectors.json').read_text())['wire']:
        if v['kind']=='leaf': actual=atom(v['id'],bytes.fromhex(v['payload_hex']))
        elif v['kind']=='parent': actual=v['id'].encode()+struct.pack('<I',0x80000000|v['count'])
        ok(actual.hex()==v['wire_hex'],v['label'])
    u=uuid.UUID('00112233-4455-6677-8899-aabbccddeeff')
    ok(u.bytes.hex()=='00112233445566778899aabbccddeeff','protocol GUID order')
    ok(u.bytes_le.hex()=='33221100554477668899aabbccddeeff','hash input GUID order')
    ok(dotnet_string('あ').hex()=='03e38182','UTF8 byte count not character count')
    ok(dotnet_string('x'*127)[:1]==b'\x7f','127 string prefix')
    ok(dotnet_string('x'*128)[:2]==b'\x80\x01','128 string prefix')
    ok(ipaddress.ip_address('1.2.3.4').packed[::-1].hex()=='04030201','IPv4 reversed bytes')
    ok(ipaddress.ip_address('2001:db8::1').packed[::-1].hex()=='010000000000000000000000b80d0120','IPv6 reversed bytes')
    for v in json.loads((ROOT/'reference_vectors.json').read_text())['station_id_review_examples']:
        ok(station_id(**v['input'])==v['expected_hex'],'Station review example '+v['label'])
    plan=json.loads((ROOT/'implementation-plan.json').read_text())
    tasks=plan['tasks']; task_ids={t['id'] for t in tasks}
    ok(len(tasks)==50 and len(task_ids)==50,'50 unique tasks')
    used=set()
    for t in tasks:
        ok(set(t['depends_on'])<=task_ids,'known dependencies '+t['id'])
        ok(t['status']=='not_started' and not t['evidence'],'no fabricated completion '+t['id'])
        ok(bool(t['scope'] and t['steps'] and t['acceptance'] and t['stop_if']),'complete task contract '+t['id'])
        used.update(t['requirements'])
    ok(used==set(ids),'all requirements mapped, no undefined references')
    ready_done=set(); pending={t['id']:t for t in tasks}
    while pending:
        ready=[k for k,t in pending.items() if set(t['depends_on'])<=ready_done]
        assert ready,'dependency cycle'
        for k in ready: ready_done.add(k); del pending[k]
    ok(len(ready_done)==50,'acyclic executable task graph')
    md=(ROOT/'IMPLEMENTATION_PLAN.md').read_text()
    for t in tasks:
        ok('## '+t['id']+' — '+t['title'] in md,'MD/JSON task heading parity '+t['id'])
        ok(all(x in md for x in t['steps']+t['acceptance']+t['stop_if']),'MD/JSON contract parity '+t['id'])
    print(f'{checks} checks passed; {len(ids)} requirement IDs; {len(ev)} pinned source entries; {len(tasks)} task contracts.')
    print('Scope: document integrity and review reference encodings only. No upstream runtime or Rust interoperability tests.')
if __name__=='__main__': check()
