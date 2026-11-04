import {test} from 'node:test';
import assert from 'node:assert/strict';
import {hash} from '../src/proof.js';
import {attest} from '../src/attestation.js';
import {Authority} from '../src/authority.js';
import {fixture,key,now} from './helpers/relay.js';
test('unknown clients, fabricated attestation, bearer credentials and unknown methods cannot send',async()=>{
 const f=fixture();
 assert.equal((await f.call('/unknown-method',{})).status,404);
 assert.equal((await f.call('/v2/deliver',{}, {authorization:'Bearer '+'b'.repeat(64)})).status,403);
 assert.equal((await f.call('/v2/subscriptions',{keyId:'unknown',challenge:'a'.repeat(64),data:'e30=',assertion:'e30='})).status,401);
 assert.throws(()=>attest('e30=',f.keyId,'a'.repeat(64),now()),/InvalidAttestation/);
 assert.equal(f.calls(),0);
});
test('official assertion and Host possession are both required; tampering does not send',async()=>{
 const f=fixture(), v=f.registration();v.grant.signature='A'.repeat(86)+'==';
 assert.equal((await f.a.fetch(await f.clientRequest('/v2/subscriptions',v),f.send)).status,401);
 const request=await f.clientRequest('/v2/subscriptions',f.registration(),{data:Buffer.from('{}').toString('base64')});
 assert.equal((await f.a.fetch(request,f.send)).status,401);assert.equal(f.calls(),0);
});
test('valid route delivers with fixed Apple destination; exact replay and cross-host proofs fail',async(t)=>{
 const clock=Date.now();t.mock.method(Date,'now',()=>clock);
 const f=fixture(),token=await f.route(),r=f.hostRequest(token);
 assert.equal((await f.a.fetch(r.clone(),f.send)).status,200);
 assert.equal((await f.a.fetch(r.clone(),f.send)).status,409);
 assert.equal((await f.a.fetch(f.hostRequest(token,{signer:key()}),f.send)).status,401);
 assert.equal((await f.a.fetch(f.hostRequest(token,{created:now()-200}),f.send)).status,401);
 assert.equal((await f.a.fetch(f.hostRequest(token),f.send)).status,409);
 assert.equal(f.calls(),1);
});
test('assertions and challenges are consumed once, even with concurrent requests',async()=>{
 const f=fixture(),r=await f.clientRequest('/v2/subscriptions',f.registration());
 const results=await Promise.all([f.a.fetch(r.clone()),f.a.fetch(r.clone())]);
 assert.deepEqual(results.map(r=>r.status).sort(),[200,401]);
});
test('token rotation invalidates old routes, and revocation cannot be undone by late registration',async()=>{
 const f=fixture(),first=await f.route(),second=await f.route(2);
 assert.notEqual(first,second);assert.equal((await f.a.fetch(f.hostRequest(first),f.send)).status,410);
 assert.equal((await f.a.fetch(await f.clientRequest('/v2/revoke',{activityId:'activity'}))).status,200);
 assert.equal((await f.a.fetch(f.hostRequest(second),f.send)).status,410);
 assert.equal((await f.a.fetch(await f.clientRequest('/v2/subscriptions',f.registration(3)))).status,409);
 assert.equal(f.calls(),0);
});
test('body tampering, environment change, binding mismatch and arbitrary alert are refused',async()=>{
 const f=fixture(),token=await f.route();
 const r=f.hostRequest(token),bad=new Request(r.url,{method:'POST',headers:r.headers,body:(await r.text()).replace('working','thinking')});
 assert.equal((await f.a.fetch(bad,f.send)).status,401);
 const validate=f.hostRequest(token,{path:'/v2/validate',value:{deliveryToken:token,binding:'f'.repeat(64),environment:'production'}});
 assert.equal((await f.a.fetch(validate,f.send)).status,403);
 const original=await f.hostRequest(token).json();original.payload.aps.alert='malicious';
 assert.equal((await f.a.fetch(f.hostRequest(token,{value:original}),f.send)).status,400);
 const v=f.registration(2);v.grant.environment='sandbox';assert.equal((await f.a.fetch(await f.clientRequest('/v2/subscriptions',v),f.send)).status,400);
 assert.equal(f.calls(),0);
});
test('revocation during an in-flight APNs call never resurrects subscription',async()=>{
 const f=fixture(),token=await f.route();let release;
 const pending=f.a.fetch(f.hostRequest(token),()=>new Promise(resolve=>{release=()=>resolve(new Response(null,{status:200}));}));
 while(!release) await new Promise(r=>setTimeout(r,1));
 assert.equal((await f.a.fetch(await f.clientRequest('/v2/revoke',{activityId:'activity'}))).status,200);
 release();assert.equal((await pending).status,200);
 assert.equal((await f.a.fetch(f.hostRequest(token),f.send)).status,410);assert.equal(f.calls(),0);
});
test('storage failure, disabled device, exhausted quotas and expired subscriptions fail closed',async()=>{
 const f=fixture(),token=await f.route();f.a.store.put('quota:delivery',{window:Math.floor(now()/60),count:6000,expires:now()+60},now()+60);
 assert.equal((await f.a.fetch(f.hostRequest(token),f.send)).status,429);
 const client=f.a.store.get('device:'+f.keyId);f.a.store.put('device:'+f.keyId,{...client,disabled:true});
 assert.equal((await f.a.fetch(f.hostRequest(token),f.send)).status,410);
 f.a.store.get=()=>{throw new Error('offline')};assert.equal((await f.a.fetch(f.hostRequest(token),f.send)).status,503);assert.equal(f.calls(),0);
});

test('emergency policy rejects global, device and Host delivery while retaining revocation',async()=>{
 for(const mode of ['disabled','revokedDevices','revokedHosts','malformed']) {
  const f=fixture(),token=await f.route();const p={disabled:false,revokedDevices:[],revokedHosts:[]};
  if(mode==='disabled')p.disabled=true;
  if(mode==='revokedDevices')p.revokedDevices=[hash(f.keyId)];
  if(mode==='revokedHosts')p.revokedHosts=[hash(Buffer.from(f.host.raw,'base64'))];
  f.a.env.NOTIFICATION_POLICY=mode==='malformed'?'{}':JSON.stringify(p);
  assert.equal((await f.a.fetch(f.hostRequest(token),f.send)).status,['disabled','malformed'].includes(mode)?503:403);
  assert.equal(f.calls(),0);
  assert.equal((await f.a.fetch(f.hostRequest(token,{path:'/v2/retire',value:{deliveryToken:token}}),f.send)).status,200);
 }
});

test('challenge throttling returns a retry window and retains the limit',async(t)=>{
  const clock=Date.now();t.mock.method(Date,'now',()=>clock);
  const f=fixture();
  const at=now();
  f.a.store.put('quota:challenge:'+hash(f.keyId),{window:Math.floor(at/60),count:12,expires:at+120},at+120);
  const r=await f.call('/v2/challenges',{keyId:f.keyId,purpose:'subscribe'});
  assert.equal(r.status,429);const value=await r.json();
  assert.equal(value.error,'RateLimited');assert.ok(value.retryAfter>=1 && value.retryAfter<=60);
  assert.equal(r.headers.get('retry-after'),String(value.retryAfter));
});

test('slow challenge bodies consume the processing window without resetting its quota',async(t)=>{
 const boundary=Math.floor(Date.now()/60000)*60+60;
 let clock=boundary-1;
 t.mock.method(Date,'now',()=>clock*1000);
 for(const used of [0,12]) {
  clock=boundary-1;
  const f=fixture(),value={keyId:f.keyId,purpose:'subscribe'};
  let release;
  const body=new ReadableStream({start(controller){release=()=>{
   controller.enqueue(new TextEncoder().encode(JSON.stringify(value)));controller.close();
  };}});
  const pending=f.a.fetch(new Request(f.request('/v2/challenges',value),{body,duplex:'half'}),f.send);
  clock=boundary;
  for(let i=0;i<used;i++) assert.equal((await f.call('/v2/challenges',value)).status,200);
  release();
  assert.equal((await pending).status,used===12?429:200);
  for(let i=used+1;i<12;i++) assert.equal((await f.call('/v2/challenges',value)).status,200);
  assert.equal((await f.call('/v2/challenges',value)).status,429);
  clock+=60;
  assert.equal((await f.call('/v2/challenges',value)).status,200);
 }
});


test('concurrent delivery returns the busy deadline without advancing the route',async()=>{
 const f=fixture(),token=await f.route(),first=f.hostRequest(token);let release;
 const pending=f.a.fetch(first.clone(),()=>new Promise(resolve=>{release=()=>resolve(new Response(null,{status:200}));}));
 while(!release) await new Promise(r=>setTimeout(r,1));
 const next=await first.json();next.payload.aps.timestamp++;
 const busy=await f.a.fetch(f.hostRequest(token,{value:next}),f.send);
 assert.equal(busy.status,429);assert.ok((await busy.json()).retryAfter>0);
 release();assert.equal((await pending).status,200);
 assert.equal((await f.a.fetch(f.hostRequest(token,{value:next}),f.send)).status,200);
});
test('replay protection and revocation survive Authority reconstruction',async()=>{
 const f=fixture(),token=await f.route(),request=f.hostRequest(token);
 assert.equal((await f.a.fetch(request.clone(),f.send)).status,200);
 const {Authority}=await import('../src/authority.js');
 const restarted=new Authority(f.a.store.storage,f.a.env);
 assert.equal((await restarted.fetch(request.clone(),f.send)).status,409);
 assert.equal((await restarted.fetch(f.hostRequest(token,{path:'/v2/retire',value:{deliveryToken:token}}),f.send)).status,200);
 const again=new Authority(f.a.store.storage,f.a.env);
 assert.equal((await again.fetch(f.hostRequest(token),f.send)).status,410);
});


test('per-route throttling does not request a Host-wide pause',async(t)=>{
 const clock=Date.now();t.mock.method(Date,'now',()=>clock);
 const f=fixture(),token=await f.route(),at=now();
 f.a.store.put('quota:route-delivery:'+f.a.store.get('route:'+token).slot,{window:Math.floor(at/60),count:20,expires:at+120},at+120);
 const r=await f.a.fetch(f.hostRequest(token),f.send);
 assert.equal(r.status,429);assert.equal((await r.json()).retryScope,'subscription');assert.equal(f.calls(),0);
});

test('sparse deliveries survive reconstruction throughout an eight-hour subscription',async(t)=>{
 let clock=Math.floor(Date.now()/86400000)*86400+60;
 t.mock.method(Date,'now',()=>clock*1000);
 const f=fixture(),token=await f.route();
 for(let i=1;i<1920;i++) {
  clock+=15;
  const a=new Authority(f.a.store.storage,f.a.env);
  const r=await a.fetch(f.hostRequest(token),f.send);
  assert.equal(r.status,200,`delivery ${i}: ${await r.clone().text()}`);
  if(i%20===0) a.store.clean(clock);
 }
 assert.equal(f.calls(),1919);
 assert.equal(new Authority(f.a.store.storage,f.a.env).store.get('quota:daily-delivery').count,1919);
});

test('APNs device throttling retains subscription scope and the server retry delay',async()=>{
 const f=fixture(),token=await f.route();
 const r=await f.a.fetch(f.hostRequest(token),async()=>Response.json({reason:'TooManyRequests'},
  {status:429,headers:{'retry-after':'60'}}));
 assert.equal(r.status,429);
 assert.equal(r.headers.get('retry-after'),'60');
 assert.deepEqual(await r.json(),{error:'APNsRateLimited',retryAfter:60,retryScope:'subscription'});
 const retry=await f.hostRequest(token).json();retry.payload.aps.timestamp++;
 assert.equal((await f.a.fetch(f.hostRequest(token,{value:retry}),f.send)).status,200);
});

test('APNs provider throttling, credentials and outages remain service failures',async()=>{
 for(const [status,reason,delay] of [[429,'TooManyProviderTokenUpdates',60],[429,'Unknown',60],
  [503,'ServiceUnavailable',60],[403,'ExpiredProviderToken',900]]) {
  const f=fixture(),token=await f.route();
  const r=await f.a.fetch(f.hostRequest(token),async()=>Response.json({reason},
   {status,headers:{'retry-after':'60'}}));
  assert.equal(r.status,503);
  assert.equal(r.headers.get('retry-after'),String(delay));
  assert.deepEqual(await r.json(),{error:'APNsUnavailable',retryAfter:delay});
 }
});
