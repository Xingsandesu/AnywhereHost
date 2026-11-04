import {test} from 'node:test';
import assert from 'node:assert/strict';
import {sign} from 'node:crypto';
import {fixture,key,now} from './helpers/relay.js';
import {Authority} from '../src/authority.js';
import {grantBase,hash} from '../src/proof.js';
const slot=f=>'slot:'+hash(f.keyId+':activity');
async function update(f,token,phase,timestamp=now()) {
 const v=await f.hostRequest(token).json();
 Object.assign(v.payload.aps,{timestamp,event:['completed','failed','stopped'].includes(phase)?'end':'update'});
 v.payload.aps['content-state'].phase=phase;return v;
}
function grant(f,host,activityId) {
 const v=f.registration();v.activityId=activityId;Object.assign(v.grant,{hostPublicKey:host.raw,activityId});
 v.grant.signature=sign('sha256',Buffer.from(grantBase(v.grant)),{key:host.privateKey,dsaEncoding:'ieee-p1363'}).toString('base64');return v;
}
test('1000 fabricated attestations never spend enrollment quota; changing keys cannot evade source limits',async(t)=>{
 let clock=Math.floor(Date.now()/86400000)*86400+60;t.mock.method(Date,'now',()=>clock*1000);const f=fixture();
 for(let i=0;i<1000;i++) {
  if(i%6===0) clock+=60;
  const keyId=Buffer.alloc(32).fill(i.toString()).toString('base64');
  const challenge=await f.call('/v2/challenges',{keyId,purpose:'attest'});
  const r=await f.call('/v2/attestations',{keyId,challenge:(await challenge.json()).challenge,attestation:'e30='},{'cf-connecting-ip':'192.0.2.1'});
  assert.equal(r.status,401);
 }
 assert.equal(f.a.store.get('quota:daily-attestations'),null);clock+=60;
 for(let i=0;i<7;i++) {
  const r=await f.call('/v2/attestations',{keyId:Buffer.alloc(32,i).toString('base64'),challenge:'a'.repeat(64),attestation:'e30='},{'cf-connecting-ip':'192.0.2.1'});
  assert.equal(r.status,i<6?401:429);
 }
 assert.equal((await f.call('/v2/attestations',{keyId:f.keyId,challenge:'a'.repeat(64),attestation:'e30='},{'cf-connecting-ip':'192.0.2.2'})).status,401);
 assert.equal(f.a.store.get('quota:daily-attestations'),null);
});
test('twenty sends and five activity rejections cost exactly twenty daily deliveries',async(t)=>{
 const at=Math.floor(Date.now()/60000)*60+1;t.mock.method(Date,'now',()=>at*1000);const f=fixture(),token=await f.route();
 for(let i=0;i<25;i++) assert.equal((await f.a.fetch(f.hostRequest(token,{value:await update(f,token,'working',at+i)}),f.send)).status,i<20?200:429);
 assert.equal(f.calls(),20);assert.equal(f.a.store.get('quota:daily-delivery').count,20);
 const rotated=await f.route(2);
 assert.equal((await f.a.fetch(f.hostRequest(rotated,{value:await update(f,rotated,'working',at+26)}),f.send)).status,429);
 assert.equal(f.a.store.get('quota:daily-delivery').count,20);
});
test('nonce replay and unavailable provider do not consume delivery quota',async()=>{
 const f=fixture(),token=await f.route(),request=f.hostRequest(token),nonce=request.headers.get('signature-input').match(/nonce="([^"]+)"/)[1];
 f.a.store.consume('nonce:'+hash(Buffer.from(f.host.raw,'base64'))+':'+nonce,now()+120);
 assert.equal((await f.a.fetch(request,f.send)).status,409);assert.equal(f.a.store.get('quota:daily-delivery'),null);
 f.a.env.APNS_CONFIG='{}';assert.equal((await f.a.fetch(f.hostRequest(token),f.send)).status,503);
 assert.equal(f.a.store.get('quota:daily-delivery'),null);assert.equal(f.calls(),0);
});
test('priority follows delivered transitions across rotation and restart',async()=>{
 const f=fixture();let token=await f.route(),at=now();const priorities=[];
 const send=async(_,init)=>{priorities.push(init.headers['apns-priority']);return new Response(null,{status:200});};
 for(const phase of ['approval','approval','working','approval']) assert.equal((await f.a.fetch(f.hostRequest(token,{value:await update(f,token,phase,at++)}),send)).status,200);
 token=await f.route(2);const restarted=new Authority(f.a.store.storage,f.a.env);
 assert.equal((await restarted.fetch(f.hostRequest(token,{value:await update(f,token,'approval',at++)}),send)).status,200);
 assert.deepEqual(priorities,['10','5','5','10','5']);
});
test('failed approval attempts preserve urgency and count actual APNs attempts',async()=>{
 const f=fixture(),token=await f.route(),at=now(),priorities=[];
 for(let i=0;i<2;i++) {
  const r=await f.a.fetch(f.hostRequest(token,{value:await update(f,token,'approval',at+i)}),async(_,init)=>{
   priorities.push(init.headers['apns-priority']);return new Response(null,{status:i===0?503:200});
  });assert.equal(r.status,i===0?503:200);
 }
 assert.deepEqual(priorities,['10','10']);assert.equal(f.a.store.get('quota:daily-delivery').count,2);
});
test('end is irreversible across version rotation, failed delivery and restart',async()=>{
 for(const status of [200,503]) {
  const f=fixture(),token=await f.route(),at=now();
  await f.a.fetch(f.hostRequest(token,{value:await update(f,token,'completed',at)}),async()=>new Response(null,{status}));
  const restarted=new Authority(f.a.store.storage,f.a.env);
  assert.equal((await restarted.fetch(await f.clientRequest('/v2/subscriptions',f.registration(2)),f.send)).status,409);
  assert.equal((await restarted.fetch(f.hostRequest(token,{value:await update(f,token,'working',at+1)}),f.send)).status,409);
  if(status===503) assert.equal((await restarted.fetch(f.hostRequest(token,{value:await update(f,token,'completed',at+2)}),f.send)).status,200);
 }
});
test('rotation during end delivery is rejected and cannot revive the activity',async()=>{
 const f=fixture(),token=await f.route();let release;
 const pending=f.a.fetch(f.hostRequest(token,{value:await update(f,token,'completed')}),()=>new Promise(resolve=>{release=()=>resolve(new Response(null,{status:200}));}));
 while(!release) await new Promise(r=>setTimeout(r,1));
 assert.equal((await f.a.fetch(await f.clientRequest('/v2/subscriptions',f.registration(2)),f.send)).status,409);
 release();assert.equal((await pending).status,200);assert.equal(f.a.store.get(slot(f)).ended,true);
});
test('sixteen active Host limit releases after expiry, retirement, revocation or end',async(t)=>{
 let clock=Math.floor(Date.now()/60000)*60+1;t.mock.method(Date,'now',()=>clock*1000);
 for(const release of ['expiry','retire','revoke','end']) {
  const f=fixture(),hosts=Array.from({length:17},()=>key()),tokens=[];
  for(let i=0;i<16;i++) tokens.push(f.a.store.transaction(()=>f.a.subscribe(f.keyId,grant(f,hosts[i],'activity-'+i),now())).deliveryToken);
  assert.throws(()=>f.a.store.transaction(()=>f.a.subscribe(f.keyId,grant(f,hosts[16],'activity-16'),now())),/HostLimit/);
  if(release==='expiry') clock+=28801;
  if(release==='revoke') f.a.store.transaction(()=>f.a.revoke(f.keyId,{activityId:'activity-0'},now()));
  if(release==='retire') assert.equal((await f.a.fetch(f.hostRequest(tokens[0],{signer:hosts[0],path:'/v2/retire',value:{deliveryToken:tokens[0]}}),f.send)).status,200);
  if(release==='end') assert.equal((await f.a.fetch(f.hostRequest(tokens[0],{signer:hosts[0],value:await update(f,tokens[0],'completed')}),f.send)).status,200);
  assert.ok(f.a.store.transaction(()=>f.a.subscribe(f.keyId,grant(f,hosts[16],'activity-16'),now())).deliveryToken);
 }
});
test('rotation preserves original expiry and ordering',async(t)=>{
 let clock=Math.floor(Date.now()/60000)*60+1;t.mock.method(Date,'now',()=>clock*1000);
 const f=fixture(),token=await f.route(),first=f.a.store.get('route:'+token);
 assert.equal((await f.a.fetch(f.hostRequest(token),f.send)).status,200);clock+=60;
 const rotated=await f.route(2);assert.equal(f.a.store.get('route:'+rotated).expires,first.expires);
 assert.equal((await f.a.fetch(f.hostRequest(rotated,{value:await update(f,rotated,'working',clock-60)}),f.send)).status,409);
});
test('old observations remain valid; Host cannot specify APNs timing or priority fields',async()=>{
 const f=fixture(),token=await f.route(),v=await update(f,token,'disconnected');let sent;
 assert.equal((await f.a.fetch(f.hostRequest(token,{value:v}),async(_,init)=>{sent=JSON.parse(init.body);return new Response(null,{status:200});})).status,200);
 assert.equal(sent.aps['stale-date'],v.payload.aps.timestamp+180);assert.equal(sent.aps['content-state'].updatedAt,v.payload.aps['content-state'].updatedAt);
 for(const field of ['stale-date','dismissal-date','relevance-score']) {
  const bad=structuredClone(v);bad.payload.aps.timestamp++;bad.payload.aps[field]=field==='relevance-score'?100:now()-181;
  assert.equal((await f.a.fetch(f.hostRequest(token,{value:bad}),f.send)).status,400);
 }
});

test('global rejection rolls back subscription budgets, nonce and activity state',async(t)=>{
 const at=Math.floor(Date.now()/60000)*60+1;t.mock.method(Date,'now',()=>at*1000);
 const f=fixture(),token=await f.route(),before=f.a.store.get(slot(f));
 f.a.store.put('quota:daily-delivery',{window:Math.floor(at/86400),count:100000,expires:at+86400},at+86400);
 const request=f.hostRequest(token);
 for(let i=0;i<5;i++) assert.equal((await f.a.fetch(request.clone(),f.send)).status,429);
 assert.equal(f.a.store.get('quota:route-delivery:'+slot(f)),null);
 assert.equal(f.a.store.get('quota:device-delivery:'+hash(f.keyId)),null);
 assert.deepEqual(f.a.store.get(slot(f)),before);assert.equal(f.calls(),0);
 // Restoring budget in the fixture proves the failed transaction did not consume the nonce.
 f.a.store.delete('quota:daily-delivery');
 assert.equal((await f.a.fetch(request,f.send)).status,200);
});
