import assert from 'node:assert/strict';
import {DatabaseSync} from 'node:sqlite';
import {generateKeyPairSync,createHash,sign,randomBytes} from 'node:crypto';
import cbor from 'cbor';
import {Authority} from '../../src/authority.js';
import {ORIGIN,grantBase,hash} from '../../src/proof.js';
export function storage() {
  const db=new DatabaseSync(':memory:');
  return {sql:{exec(sql,...args){const q=db.prepare(sql);const rows=q.columns().length?q.all(...args):(q.run(...args),[]);return {toArray:()=>rows,one:()=>{assert.equal(rows.length,1);return rows[0]}};}},transactionSync(fn){db.exec('BEGIN');try{const r=fn();db.exec('COMMIT');return r;}catch(e){db.exec('ROLLBACK');throw e;}}};
}
export function key() {const k=generateKeyPairSync('ec',{namedCurve:'P-256'});const j=k.publicKey.export({format:'jwk'});return {...k,raw:Buffer.concat([Buffer.from([4]),Buffer.from(j.x,'base64url'),Buffer.from(j.y,'base64url')]).toString('base64')};}
const b64=v=>createHash('sha256').update(v).digest('base64');
export const now=()=>Math.floor(Date.now()/1000);
export function fixture(options={}) {
  const host=key(),device=key(),provider=key();let counter=0,calls=0;
  const keyId=b64(Buffer.from(device.raw,'base64'));
  const a=new Authority(storage(),{APNS_CONFIG:JSON.stringify({production:{keyId:'ABCDEFGHIJ',teamId:'775Y7KV7KU',privateKey:provider.privateKey.export({type:'pkcs8',format:'pem'})}})});
  // Test setup inserts a verified device; the production API has no bypass or injected verifier.
  a.store.put('device:'+keyId,{publicKey:device.publicKey.export({type:'spki',format:'pem'}),counter:0,disabled:false});
  const request=(path,v,headers={})=>new Request(ORIGIN+path,{method:'POST',headers:{'content-type':'application/json',...headers},body:JSON.stringify(v)});
  const send=async(url,init)=>{calls++;assert.equal(url,'https://api.push.apple.com/3/device/'+'d'.repeat(64));assert.equal(init.headers['apns-topic'],'com.jintongshu.AnyWhereCoding.push-type.liveactivity');assert.equal(init.redirect,'manual');return new Response(null,{status:200});};
  const call=options.call ?? ((path,v,h)=>a.fetch(request(path,v,h),send));
  async function clientRequest(path,value,modify={}) {
    const purpose=path==='/v2/revoke'?'revoke':'subscribe';
    const r=await call('/v2/challenges',{keyId,purpose});assert.equal(r.status,200);
    const {challenge}=await r.json(); const data=Buffer.from(JSON.stringify(value));
    const header=Buffer.alloc(37);Buffer.from(b64('775Y7KV7KU.com.jintongshu.AnyWhereCoding'),'base64').copy(header);header.writeUInt32BE(++counter,33);
    const auth=Buffer.concat([header,await cbor.encodeAsync({validationCategory:Buffer.from([2,0,0,0]),bundleVersion:'1'})]);
    const message=Buffer.concat([Buffer.from(`Anywhere-Relay-v2\n${ORIGIN}${path}\n${challenge}\n`),data]);
    const nonce=createHash('sha256').update(Buffer.concat([auth,Buffer.from(b64(message),'base64')])).digest();
    const assertion=(await cbor.encodeAsync({signature:sign('sha256',nonce,device.privateKey),authenticatorData:auth})).toString('base64');
    return request(path,{keyId,challenge,data:data.toString('base64'),assertion,...modify});
  }
  function registration(version=1) {
    const grant={hostPublicKey:host.raw,keyId,binding:'c'.repeat(64),environment:'production',hostId:'e'.repeat(64),activityId:'activity',tokenVersion:version,tokenDigest:hash('d'.repeat(64))};
    grant.signature=sign('sha256',Buffer.from(grantBase(grant)),{key:host.privateKey,dsaEncoding:'ieee-p1363'}).toString('base64');
    return {grant,activityId:'activity',pushToken:'d'.repeat(64),tokenVersion:version};
  }
  async function route(version=1) {const r=await a.fetch(await clientRequest('/v2/subscriptions',registration(version)),send);assert.equal(r.status,200,await r.clone().text());return (await r.json()).deliveryToken;}
  function hostRequest(token,options={}) {
    const path=options.path??'/v2/deliver';
    const value=options.value??{deliveryToken:token,payload:{aps:{timestamp:now(),event:'update','content-state':{phase:'working',startedAt:800000000,endedAt:null,updatedAt:800000000,completedTools:0,revision:1}}}};
    const raw=JSON.stringify(value),digest='sha-256=:'+b64(raw)+':',created=options.created??now();
    const signer=options.signer??host;
    const params=`("@method" "@target-uri" "content-digest");created=${created};expires=${created+90};nonce="${randomBytes(16).toString('hex')}";keyid="${hash(Buffer.from(signer.raw,'base64'))}";alg="ecdsa-p256-sha256";tag="anywhere-relay-v2"`;
    const base=`"@method": POST\n"@target-uri": ${ORIGIN}${path}\n"content-digest": ${digest}\n"@signature-params": ${params}`;
    return request(path,value,{'content-digest':digest,'signature-input':'host='+params,'signature':'host=:'+sign('sha256',Buffer.from(base),{key:signer.privateKey,dsaEncoding:'ieee-p1363'}).toString('base64')+':'});
  }
  return {a,keyId,host,device,call,request,clientRequest,registration,route,hostRequest,send,calls:()=>calls};
}
