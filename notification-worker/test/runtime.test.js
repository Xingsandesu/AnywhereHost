import {test} from 'node:test';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {mkdtemp,writeFile,rm,readFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {fileURLToPath} from 'node:url';
import {createServer} from 'node:net';
import {networkFixture} from './helpers/network.js';
import {fixture as signedFixture} from './helpers/relay.js';
test('workerd verifies signed registration, Host delivery, replay rejection and revocation without cloud credentials',{timeout:45000},async(t)=>{
 const dir=await mkdtemp(join(tmpdir(),'relay-runtime-'));let child;
 try {
  const listener=createServer();await new Promise(r=>listener.listen(0,'127.0.0.1',r));const port=listener.address().port;await new Promise(r=>listener.close(r));
  const call=(path,data)=>fetch(`http://127.0.0.1:${port}${path}`,{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify(data)});
  const signed=signedFixture({call}),network=networkFixture();
  const send=request=>fetch(`http://127.0.0.1:${port}${new URL(request.url).pathname}`,request);
  const config=join(dir,'wrangler.json');
  await writeFile(config,JSON.stringify({name:'relay-test',main:fileURLToPath(new URL('fixtures/runtime-entry.js',import.meta.url)),compatibility_date:'2026-11-01',compatibility_flags:['nodejs_compat'],vars:{NETWORK_POLICY:network.a.env.NETWORK_POLICY,NETWORK_SIGNING_KEY:network.a.env.NETWORK_SIGNING_KEY,TEST_NETWORK:JSON.stringify({keyId:network.keyId,publicKey:network.app.publicKey.export({type:'spki',format:'pem'})}),APNS_CONFIG:signed.a.env.APNS_CONFIG,TEST_DEVICE:JSON.stringify({keyId:signed.keyId,publicKey:signed.device.publicKey.export({type:'spki',format:'pem'})})},durable_objects:{bindings:[{name:'AUTHORITY',class_name:'RuntimeAuthority'}]},migrations:[{tag:'test',new_sqlite_classes:['RuntimeAuthority']}]}));
  child=spawn(process.execPath,[fileURLToPath(new URL('../node_modules/wrangler/bin/wrangler.js',import.meta.url)),'dev','--config',config,'--local','--ip','127.0.0.1','--port',String(port),'--persist-to',join(dir,'state')],{stdio:['ignore','pipe','pipe']});
  await new Promise((resolve,reject)=>{
   const timeout=setTimeout(()=>reject(new Error('workerd startup timed out')),30000);let output='';
   const data=b=>{output+=b.toString();if(output.includes('Ready on')){clearTimeout(timeout);resolve();}};
   child.stdout.on('data',data);child.stderr.on('data',data);child.once('exit',code=>{clearTimeout(timeout);reject(new Error(`workerd exited ${code}: ${output.slice(-2000)}`));});
  });
  assert.equal((await (await call('/test/alarm',{})).json()).at,null);
  const networkRequest=await network.lease();
  const networkBody=await networkRequest.clone().json();
  // Install this test's one-time challenge in the isolated runtime only.
  await call('/test/network-challenge',networkBody);
  const admitted=await send(networkRequest.clone());assert.equal(admitted.status,200,await admitted.clone().text());
  const certificate=(await admitted.json()).token;assert.equal(certificate.split('.').length,3);
  assert.equal((await send(networkRequest)).status,401);

  const statusBefore=await (await call('/test/metrics',{})).json();
  for(let i=0;i<10;i++) assert.equal((await call('/v2/status',{})).status,200);
  assert.deepEqual(await (await call('/test/metrics',{})).json(),statusBefore);
  const fixture=JSON.parse(await readFile(new URL('fixtures/attestation-production.json',import.meta.url)));
  const r=await call('/v2/challenges',{keyId:fixture.keyId,purpose:'attest'});assert.equal(r.status,200,await r.clone().text());const {challenge}=await r.json();
  assert.match(challenge,/^[a-f0-9]{64}$/);
  assert.ok((await (await call('/test/alarm',{})).json()).at>Date.now());
  assert.equal((await call('/v2/attestations',{keyId:fixture.keyId,challenge,attestation:fixture.attestation})).status,401);
  assert.equal((await call('/v2/deliver',{deliveryToken:'0'.repeat(128),payload:{}})).status,410);
  const tampered=signed.registration();tampered.grant.signature='A'.repeat(86)+'==';
  assert.equal((await send(await signed.clientRequest('/v2/subscriptions',tampered))).status,401);
  const registration=await signed.clientRequest('/v2/subscriptions',signed.registration());
  const registered=await send(registration.clone());
  assert.equal(registered.status,200,await registered.clone().text());
  const {deliveryToken}=await registered.json();
  assert.equal((await send(registration.clone())).status,401);
  const validated=await send(signed.hostRequest(deliveryToken,{path:'/v2/validate',value:{deliveryToken,binding:'c'.repeat(64),environment:'production'}}));
  assert.equal(validated.status,200,await validated.clone().text());
  const before=await (await call('/test/metrics',{})).json();
  const delivery=signed.hostRequest(deliveryToken);
  const delivered=await send(delivery.clone());
  assert.equal(delivered.status,200,await delivered.clone().text());
  assert.equal((await delivered.json()).accepted,true);
  assert.equal((await send(delivery.clone())).status,409);
  // Equal state can be a freshness heartbeat; every send still needs a unique timestamp.
  const initial=await delivery.clone().json();
  for(let i=1;i<20;i++) {
    initial.payload.aps.timestamp++;
    const r=await send(signed.hostRequest(deliveryToken,{value:initial}));
    assert.equal(r.status,200,await r.clone().text());
  }
  const after=await (await call('/test/metrics',{})).json();
  const writes=after.written-before.written;
  t.diagnostic(`20 APNs deliveries: ${writes} SQL rows written (workerd cursor metrics)`);
  assert.ok(writes<=150,`write amplification regression: ${writes}`);
  const revoked=await send(await signed.clientRequest('/v2/revoke',{activityId:'activity'}));
  assert.equal(revoked.status,200,await revoked.clone().text());
  assert.equal((await send(signed.hostRequest(deliveryToken))).status,410);
 } finally {
  if(child && child.exitCode===null){child.kill('SIGTERM');await new Promise(r=>child.once('exit',r));}
  await rm(dir,{recursive:true,force:true});
 }
});
