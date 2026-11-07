import {test} from 'node:test';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {developmentSessions} from '../src/releases.js';
import {networkFixture} from './helpers/network.js';
const now=Math.floor(Date.now()/1000);
const hash=value=>createHash('sha256').update(value).digest('hex');
const grant=f=>({codeHash:'a'.repeat(64),build:57,keyHash:hash(f.keyId),endpointId:f.endpointId,expiresAt:now+300});

test('development grants are explicit, bounded, unique and expire automatically',()=>{
 assert.deepEqual(developmentSessions(undefined,now),[]);
 const value={codeHash:'a'.repeat(64),build:57,keyHash:'b'.repeat(64),endpointId:'c'.repeat(64),expiresAt:now+300};
 assert.deepEqual(developmentSessions(JSON.stringify([value]),now),[value]);
 assert.deepEqual(developmentSessions(JSON.stringify([value]),now+300),[]);
 for(const input of ['{}','broken',JSON.stringify([value,value]),JSON.stringify([{...value,extra:true}]),JSON.stringify([{...value,expiresAt:now+7*86400+1}]),JSON.stringify([{...value,build:50}]),JSON.stringify([{...value,keyHash:'unknown'}])]) {
  assert.throws(()=>developmentSessions(input,now),/DevelopmentGrantUnavailable/);
 }
});

test('development assertions require an enrolled key, exact Mac endpoint and code; signature and replay checks still apply',async()=>{
 const f=networkFixture('host');
 assert.notEqual((await f.a.fetch(await f.lease({}, {category:3}))).status,200);
 const entry=grant(f);
 f.a.env.HOST_DEVELOPMENT_SESSIONS=JSON.stringify([entry]);
 const request=await f.lease({}, {category:3});
 const response=await f.a.fetch(request.clone());
 assert.equal(response.status,200);
 assert.equal((await response.json()).expiresAt,entry.expiresAt);
 assert.notEqual((await f.a.fetch(request)).status,200);
 // A debug grant must not displace the approved production build path.
 assert.equal((await f.a.fetch(await f.lease())).status,200);
 assert.notEqual((await f.a.fetch(await f.lease({}, {category:3,codeHash:'d'.repeat(64)}))).status,200);
 for(const change of [{keyHash:'d'.repeat(64)},{endpointId:'d'.repeat(64)},{expiresAt:now-1}]) {
  f.a.env.HOST_DEVELOPMENT_SESSIONS=JSON.stringify([{...entry,...change}]);
  assert.notEqual((await f.a.fetch(await f.lease({}, {category:3}))).status,200);
 }
 f.a.env.HOST_DEVELOPMENT_SESSIONS=JSON.stringify([entry]);
 const tampered=await (await f.lease({}, {category:3})).json();tampered.assertion='AA==';
 assert.notEqual((await f.a.fetch(f.request('leases',tampered))).status,200);
 const policy=JSON.parse(f.a.env.NETWORK_POLICY);policy.revokedHostCodeHashes=[entry.codeHash];f.a.env.NETWORK_POLICY=JSON.stringify(policy);
 assert.notEqual((await f.a.fetch(await f.lease({}, {category:3}))).status,200);
});

test('temporary Mac grants cannot authorize iOS development assertions',async()=>{
 const f=networkFixture('client');
 f.a.env.HOST_DEVELOPMENT_SESSIONS=JSON.stringify([grant(f)]);
 assert.notEqual((await f.a.fetch(await f.lease({}, {category:3}))).status,200);
});
