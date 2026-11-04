import {test} from 'node:test';
import assert from 'node:assert/strict';
import {generateKeyPairSync,createHash,sign,verify} from 'node:crypto';
import cbor from 'cbor';
import {storage,key} from './helpers/relay.js';
import {NetworkAuthority,ISSUER,settings} from '../src/network.js';
import {extensions} from '../src/attestation.js';
const hash=v=>createHash('sha256').update(v).digest();
import {networkFixture as fixture} from './helpers/network.js';
test('official app certificates bind endpoint, role, expiry and discovered relays; replay is denied',async()=>{
 for(const role of ['host','client']) {
  const f=fixture(role),r=await f.lease(),response=await f.a.fetch(r.clone());assert.equal(response.status,200,await response.clone().text());
  const {token}=await response.json(),[h,p,s]=token.split('.');
  assert.equal(verify(null,Buffer.from(h+'.'+p),f.issuer.publicKey,Buffer.from(s,'base64url')),true);
  const claims=JSON.parse(Buffer.from(p,'base64url'));assert.equal(claims.sub,f.endpointId);assert.equal(claims.role,role);assert.equal(claims.exp-claims.iat,900);
  assert.equal((await f.a.fetch(r)).status,401);
 }
});
test('unknown app, endpoint forgery, role substitution, revoked app and missing policy fail closed',async()=>{
 const f=fixture();
 assert.equal((await f.a.fetch(await f.lease({endpointId:'a'.repeat(64)}))).status,401);
 const request=await f.lease(),value=await request.json();value.role='host';assert.equal((await f.a.fetch(f.request('leases',value))).status,401);
 f.a.env.NETWORK_POLICY=JSON.stringify({disabled:false,revokedKeys:[hash(f.keyId).toString('hex')],relays:['https://relay.example.com/']});
 assert.equal((await f.a.fetch(await f.lease())).status,403);
 delete f.a.env.NETWORK_POLICY;assert.equal((await f.a.fetch(f.request('challenges',{}))).status,503);
 assert.throws(()=>settings({NETWORK_POLICY:JSON.stringify({disabled:false,revokedKeys:[],relays:['http://relay.example.com/']})}));
});
test('distribution categories are role-specific and missing signed extensions fail',async()=>{
 const auth=Buffer.alloc(37);
 const encoded=async category=>Buffer.concat([auth,await cbor.encodeAsync({validationCategory:category,bundleVersion:'51'})]);
 const six=await encoded(6),two=await encoded(2),three=await encoded(3);
 assert.throws(()=>extensions(six,37,'host'));
 assert.throws(()=>extensions(six,37,'client'));
 assert.throws(()=>extensions(two,37,'host'));
 assert.throws(()=>extensions(three,37,'client'));
 assert.throws(()=>extensions(auth,37,'host'));
});

test('Mac signed code must match an approved build, including assertions after revocation',async()=>{
 const codeHash='a'.repeat(64),builds=[{codeHash,build:51}];
 const properties={apple_validation_category_01:Buffer.from([6,0,0,0]),apple_cd_hash_type_01:Buffer.from([2]),apple_cd_hash_hash_01:Buffer.from(codeHash,'hex')};
 const check=async(values=properties,catalog=builds,min=51)=>extensions(await cbor.encodeAsync(values),0,'host',min,catalog);
 await assert.doesNotReject(()=>check());
 await assert.rejects(()=>check({...properties,apple_cd_hash_hash_01:Buffer.from(codeHash.slice(0,40),'hex')}));
 await assert.rejects(()=>check(properties,[]));await assert.rejects(()=>check(properties,builds,52));
 await assert.rejects(()=>check({...properties,bundleVersion:'52'}));
 await assert.rejects(()=>check({...properties,apple_cd_hash_type_01:Buffer.from([1])}));
 await assert.rejects(()=>check({...properties,apple_validation_category_01:2}));
 const f=fixture('host');assert.equal((await f.a.fetch(await f.lease())).status,200);
 const policy=JSON.parse(f.a.env.NETWORK_POLICY);policy.revokedHostCodeHashes=[codeHash];f.a.env.NETWORK_POLICY=JSON.stringify(policy);
 assert.equal((await f.a.fetch(await f.lease())).status,401);
});
