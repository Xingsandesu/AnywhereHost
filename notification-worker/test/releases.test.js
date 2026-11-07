import {test,mock} from 'node:test';
import assert from 'node:assert/strict';
import {generateKeyPairSync,sign,randomUUID} from 'node:crypto';
import {verifyReleaseToken,audience,localReleases} from '../src/releases.js';
import {networkFixture} from './helpers/network.js';
const pair=generateKeyPairSync('rsa',{modulusLength:2048});
const jwk={...pair.publicKey.export({format:'jwk'}),kid:'test-key',alg:'RS256',use:'sig'};
const now=Math.floor(Date.now()/1000);
const claims={iss:'https://token.actions.githubusercontent.com',aud:audience,sub:'repo:Xingsandesu@55939146/AnywhereHost@1440923483:ref:refs/heads/main',repository_id:'1440923483',repository_owner_id:'55939146',ref:'refs/heads/main',workflow_ref:'Xingsandesu/AnywhereHost/.github/workflows/release.yml@refs/heads/main',event_name:'workflow_run',iat:now,nbf:now,exp:now+300,jti:randomUUID()};
function token(changes={},key=pair.privateKey,header={}) {
 const input=[{alg:'RS256',kid:jwk.kid,...header},{...claims,...changes}].map(v=>Buffer.from(JSON.stringify(v)).toString('base64url')).join('.');
 return input+'.'+sign('RSA-SHA256',Buffer.from(input),key).toString('base64url');
}
test('local release catalog rejects malformed, ambiguous and oversized configuration',()=>{
 assert.deepEqual(localReleases(undefined),[]);
 const build={codeHash:'a'.repeat(64),build:57};
 assert.deepEqual(localReleases(JSON.stringify([build])),[build]);
 for(const value of [null,{},'invalid','{}',JSON.stringify([build,build]),JSON.stringify([{...build,build:50}]),JSON.stringify([{...build,build:100000000}]),JSON.stringify([{...build,codeHash:'unknown'}]),JSON.stringify([{...build,codeHash:[build.codeHash]}]),JSON.stringify([{...build,extra:true}]),JSON.stringify(Array(257).fill(build))]) {
  assert.throws(()=>localReleases(value),e=>e.status===503 && e.message==='ReleaseCatalogUnavailable');
 }
});
test('local releases still require signed Mac assertions and honor revocation',async()=>{
 const f=networkFixture('host');
 f.a.store.put('host-builds',[]);
 f.a.env.HOST_RELEASES=JSON.stringify([{codeHash:'a'.repeat(64),build:57}]);
 assert.equal((await f.a.fetch(await f.lease())).status,200);
 f.a.env.HOST_RELEASES=JSON.stringify([{codeHash:'b'.repeat(64),build:57}]);
 assert.notEqual((await f.a.fetch(await f.lease())).status,200);
 f.a.env.HOST_RELEASES=JSON.stringify([{codeHash:'a'.repeat(64),build:57}]);
 const policy=JSON.parse(f.a.env.NETWORK_POLICY);
 policy.revokedHostCodeHashes=['a'.repeat(64)];
 f.a.env.NETWORK_POLICY=JSON.stringify(policy);
 assert.notEqual((await f.a.fetch(await f.lease())).status,200);
});
test('release registration trusts only the signed official main workflow identity',()=>{
 assert.equal(verifyReleaseToken(token(),[jwk],now).repository_id,'1440923483');
 for(const change of [{sub:'repo:Xingsandesu/AnywhereHost:ref:refs/heads/main'},{iss:'https://evil.example'},{aud:'another-service'},{repository_id:'1'},{repository_owner_id:'1'},{ref:'refs/pull/1/merge'},{sub:'repo:Xingsandesu/AnywhereHost:pull_request'},{workflow_ref:'Xingsandesu/AnywhereHost/.github/workflows/build.yml@refs/heads/main'},{event_name:'pull_request_target'},{exp:now},{exp:now+601},{iat:now+60},{jti:''}])assert.throws(()=>verifyReleaseToken(token(change),[jwk],now));
 const foreign=generateKeyPairSync('rsa',{modulusLength:2048});
 assert.throws(()=>verifyReleaseToken(token({},foreign.privateKey),[jwk],now));
 assert.throws(()=>verifyReleaseToken(token({},pair.privateKey,{alg:'none'}),[jwk],now));
 assert.throws(()=>verifyReleaseToken(token(),[],now));
});
test('release registration is authenticated, replay-safe and cannot relabel an approved hash',async()=>{
 const fetch=mock.method(globalThis,'fetch',async url=>{
  assert.equal(url,'https://token.actions.githubusercontent.com/.well-known/jwks');
  return Response.json({keys:[jwk]});
 });
 try {
  const f=networkFixture('host'),release={codeHash:'b'.repeat(64),build:51,version:'1.1.1',sourceSha:'c'.repeat(40)};
  const request=(value,proof)=>new Request(audience,{method:'POST',headers:{'content-type':'application/json',...(proof?{authorization:'Bearer '+proof}:{})},body:JSON.stringify(value)});
  assert.equal((await f.a.fetch(request(release))).status,403);
  const proof=token();assert.equal((await f.a.fetch(request(release,proof))).status,200);
  assert.equal((await f.a.fetch(request(release,proof))).status,409);
  assert.equal((await f.a.fetch(request({...release,build:52},token({jti:randomUUID()})))).status,409);
  assert.equal((await f.a.fetch(request(release,token({jti:randomUUID()})))).status,200);
  assert.deepEqual(f.a.store.get('host-builds').find(b=>b.codeHash===release.codeHash),release);
  assert.equal(fetch.mock.calls.length,1);
 }finally{fetch.mock.restore();}
});
