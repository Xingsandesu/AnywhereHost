import {generateKeyPairSync,createHash,sign,verify} from 'node:crypto';
import cbor from 'cbor';
import {storage,key} from './relay.js';
import {NetworkAuthority,ISSUER,settings} from '../../src/network.js';
import {extensions} from '../../src/attestation.js';
const hash=v=>createHash('sha256').update(v).digest();
export function networkFixture(role='client') {
 const app=key(),endpoint=generateKeyPairSync('ed25519'),issuer=generateKeyPairSync('ed25519');
 const keyId=hash(Buffer.from(app.raw,'base64')).toString('base64'),endpointId=Buffer.from(endpoint.publicKey.export({format:'jwk'}).x,'base64url').toString('hex');
 const a=new NetworkAuthority(storage(),{NETWORK_POLICY:JSON.stringify({disabled:false,revokedKeys:[],relays:['https://relay.example.com/']}),NETWORK_SIGNING_KEY:issuer.privateKey.export({format:'pem',type:'pkcs8'})});
 a.store.put('host-builds',[{codeHash:'a'.repeat(64),build:51,version:'1.1.1',sourceSha:'b'.repeat(40)}]);
 a.store.put('key:'+keyId,{publicKey:app.publicKey.export({format:'pem',type:'spki'}),role,counter:0});let counter=0;
 const request=(path,value)=>new Request(ISSUER+'/v3/network/'+path,{method:'POST',body:JSON.stringify(value),headers:{'content-type':'application/json'}});
 async function lease(overrides={}) {
  const r=await a.fetch(request('challenges',{keyId,role,purpose:'lease'}));const {challenge}=await r.json();
  const proof=`Anywhere-Network-Endpoint-v3\n${ISSUER}\n${keyId}\n${role}\n${challenge}\n${endpointId}`;
  const data=Buffer.from(JSON.stringify({endpointId,signature:sign(null,Buffer.from(proof),endpoint.privateKey).toString('base64'),...overrides}));
  const auth=Buffer.alloc(37);hash('775Y7KV7KU.'+(role==='client'?'com.jintongshu.AnyWhereCoding':'com.jintongshu.AnywhereHost')).copy(auth);auth.writeUInt32BE(++counter,33);
  const category=Buffer.alloc(4);category.writeUInt32LE(role==='host'?6:2);
  const authenticatorData=Buffer.concat([auth,await cbor.encodeAsync(role==='host'?{apple_validation_category_01:category,apple_cd_hash_type_01:Buffer.from([2]),apple_cd_hash_hash_01:Buffer.from('a'.repeat(64),'hex')}:{validationCategory:category,bundleVersion:'51'})]);
  const message=Buffer.concat([Buffer.from(`Anywhere-Network-v3\n${ISSUER}/v3/network/leases\n${challenge}\n`),data]);
  const nonce=hash(Buffer.concat([authenticatorData,hash(message)]));
  const assertion=(await cbor.encodeAsync({authenticatorData,signature:sign('sha256',nonce,app.privateKey)})).toString('base64');
  return request('leases',{keyId,role,challenge,data:data.toString('base64'),assertion});
 }
 return {a,request,lease,keyId,endpointId,issuer,app,role};
}
