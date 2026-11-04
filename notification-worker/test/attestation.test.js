import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {X509Certificate,generateKeyPairSync,createHash,sign} from 'node:crypto';
import cbor from 'cbor';
import {verifyAttestation} from 'node-app-attest';
import {attest,assertion,decode} from '../src/attestation.js';
// Public upstream Apple-signed examples, MIT licensed in fixtures/LICENSE.
// These attest a DIFFERENT app and must never enroll into our production service.
for(const environment of ['production','development']) test(`Apple-signed ${environment} fixture verifies upstream but cannot enroll another app`,()=>{
 const f=JSON.parse(readFileSync(new URL(`fixtures/attestation-${environment}.json`,import.meta.url)));
 const challenge=Buffer.from(f.challenge,'base64');
 const bytes=Buffer.from(f.attestation,'base64');
 assert.equal(verifyAttestation({attestation:bytes,challenge,keyId:f.keyId,bundleIdentifier:'io.uebelacker.AppAttestExample',teamIdentifier:'V8H6LQ9448',allowDevelopmentEnvironment:true}).environment,environment);
 const cert=new X509Certificate(decode(bytes).attStmt.x5c[0]);
 const validTime=Math.floor(Date.parse(cert.validFrom)/1000)+1;
 assert.throws(()=>attest(f.attestation,f.keyId,challenge,validTime),/InvalidAttestation/);
 assert.throws(()=>attest(f.attestation,f.keyId,challenge,Math.floor(Date.parse(cert.validTo)/1000)+1),/InvalidAttestation/);
});
test('duplicate CBOR map keys and trailing objects are rejected',()=>{
 assert.throws(()=>decode(Buffer.from('a2616101616102','hex')));
 assert.throws(()=>decode(Buffer.from('a001','hex')));
});
test('iOS 27 signed extensions are mandatory; Apple UInt32 encoding works without the ED flag',async()=>{
 const keys=generateKeyPairSync('ec',{namedCurve:'P-256'});
 const client={publicKey:keys.publicKey.export({type:'spki',format:'pem'}),counter:0};
 const data=Buffer.from('one-use server challenge and subscription');
 const hash=b=>createHash('sha256').update(b).digest();
 async function proof(extensions,flags=0) {
  const header=Buffer.alloc(37);hash('775Y7KV7KU.com.jintongshu.AnyWhereCoding').copy(header);header[32]=flags;header.writeUInt32BE(1,33);
  const auth=Buffer.concat([header,extensions]);
  const signature=sign('sha256',hash(Buffer.concat([auth,hash(data)])),keys.privateKey);
  return (await cbor.encodeAsync({signature,authenticatorData:auth})).toString('base64');
 }
 // Apple documents both spellings for attestation and assertion respectively:
 // https://developer.apple.com/documentation/devicecheck/validating-apps-that-connect-to-your-server
 for(const category of [2,4,Buffer.from([2,0,0,0]),Buffer.from([4,0,0,0])]) {
  for(const flags of [0,0x80]) {
   const value=await proof(await cbor.encodeAsync({validationCategory:category,bundleVersion:'1'}),flags);
   assert.equal(assertion(value,data,client),1);
   assert.throws(()=>assertion(value,Buffer.from('tampered'),client),/InvalidAssertion/);
  }
 }
 const attestationNames=await proof(await cbor.encodeAsync({apple_validation_category_01:Buffer.from([2,0,0,0]),apple_bundle_version_01:'1'}));
 assert.equal(assertion(attestationNames,data,client),1);
 for(const extensions of [Buffer.alloc(0),await cbor.encodeAsync({}),await cbor.encodeAsync({validationCategory:3,bundleVersion:'1'}),await cbor.encodeAsync({validationCategory:2,bundleVersion:'0'}),await cbor.encodeAsync({validationCategory:Buffer.from([0,0,0,2]),bundleVersion:'1'}),await cbor.encodeAsync({validationCategory:3,apple_validation_category_01:2,bundleVersion:'1'}),Buffer.from('a001','hex')]) {
  const value=await proof(extensions);
  assert.throws(()=>assertion(value,data,client),/InvalidAssertion/);
 }
});
