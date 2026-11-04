import {policy} from './policy.js';
import {randomBytes} from 'node:crypto';
import {Failure,requireValue,fields,body,payload} from './validation.js';
import {ORIGIN,hash,base64,grantBase,checkSignature,verifyHost} from './proof.js';
import {attest,assertion} from './attestation.js';
import {Store} from './store.js';
import {sendAPNs} from './apns.js';
import {activity,admissible,activeHosts} from './activity.js';
import {allowed,status,reply,failure} from './status.js';
const id=()=>randomBytes(32).toString('hex');
const lifetime=28800;
export class Authority {
  constructor(storage,env) { this.store=new Store(storage);this.env=env; }
  async fetch(request, send=fetch) {
    try {
      const url=new URL(request.url), s=this.store;
      allowed(request);
      const path=url.pathname;
      if(path==='/v2/status') return status(request,this.env);
      if(!['/v2/status','/v2/challenges','/v2/attestations','/v2/subscriptions','/v2/revoke','/v2/validate','/v2/retire','/v2/deliver'].includes(path)) throw new Failure(404,'NotFound');
      s.limit('requests',6000,Math.floor(Date.now()/1000));
      if(!['/v2/revoke','/v2/retire'].includes(path)) policy(this.env);
      const {input,raw}=await body(request,path==='/v2/attestations'?32768:8192);
      // Body reads can finish out of order across quota windows. Charge all
      // subsequent synchronous operations to their processing time.
      const now=Math.floor(Date.now()/1000);
      if(path==='/v2/challenges') {
        fields(input,['keyId','purpose']); base64(input.keyId,32);
        requireValue(['attest','subscribe','revoke'].includes(input.purpose));
        s.limit('challenges',600,now);s.limit('challenge:'+hash(input.keyId),12,now);
        if(input.purpose==='attest' && s.get('device:'+input.keyId)) return reply(200,{attested:true});
        const challenge=id();s.put('challenge:'+challenge,{keyId:input.keyId,purpose:input.purpose,expires:now+120},now+120);
        return reply(200,{challenge});
      }
      if(path==='/v2/attestations') {
        // Cloudflare supplies this header at the public edge; never use a client key ID as the source identity.
        s.limit('attestation-source:'+hash(request.headers.get('cf-connecting-ip') ?? 'unknown'),6,now);
        s.limit('attestations',60,now);
        fields(input,['keyId','challenge','attestation']);
        this.challenge(input,'attest',now);
        if(s.count('device:')>=100000) throw new Failure(503,'EnrollmentCapacityReached');
        const client=attest(input.attestation,input.keyId,input.challenge,now);
        s.transaction(()=>{
          this.challenge(input,'attest',now);
          // Never reset an existing assertion counter, including retrying registration.
          if(s.get('device:'+input.keyId)) throw new Failure(409,'AlreadyAttested');
          s.limit('daily-attestations',1000,now,86400);
          s.put('device:'+input.keyId,{...client,counter:0,disabled:false});
          s.delete('challenge:'+input.challenge);
        });
        return reply(200,{registered:true});
      }
      if(path==='/v2/subscriptions' || path==='/v2/revoke') {
        fields(input,['keyId','challenge','data','assertion']);
        const purpose=path==='/v2/revoke'?'revoke':'subscribe';
        const client=s.get('device:'+input.keyId);
        if(!client || client.disabled) throw new Failure(401,'UnknownDevice');
        this.challenge(input,purpose,now);
        const data=base64(input.data);
        const count=assertion(input.assertion,Buffer.concat([Buffer.from(`Anywhere-Relay-v2\n${ORIGIN}${path}\n${input.challenge}\n`),data]),client);
        let value;try{value=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(data));}catch{throw new Failure(400,'InvalidJSON');}
        // All verification is synchronous up to this transaction. Recheck counter/challenge inside it.
        return s.transaction(()=>{
          this.challenge(input,purpose,now);
          const current=s.get('device:'+input.keyId);
          if(current.disabled || count<=current.counter) throw new Failure(409,'ReplayRejected');
          let result;
          if(purpose==='revoke') result=this.revoke(input.keyId,value,now);
          else result=this.subscribe(input.keyId,value,now);
          s.update('device:'+input.keyId,{...current,counter:count});s.delete('challenge:'+input.challenge);
          return reply(200,result);
        });
      }
      const token=input.deliveryToken;
      requireValue(typeof token==='string' && /^[a-f0-9]{128}$/.test(token));
      const route=s.get('route:'+token);
      if(!route || route.expires<=now || route.revoked) throw new Failure(410,'InvalidSubscription');
      if(path!=='/v2/retire') policy(this.env,route.keyId,route.hostPublicKey);
      const device=s.get('device:'+route.keyId);
      if(!device || device.disabled) throw new Failure(410,'InvalidSubscription');
      const proof=verifyHost(request,raw,route.hostPublicKey,now);
      s.limit('host:'+proof.keyId,600,now);
      if(path==='/v2/retire') {
        fields(input,['deliveryToken']);s.transaction(()=>{s.consume('nonce:'+proof.keyId+':'+proof.nonce,proof.expires+31);s.update('route:'+token,{...route,revoked:true});});return reply(200,{revoked:true});
      }
      if(path==='/v2/validate') {
        fields(input,['deliveryToken','binding','environment']);
        if(input.binding!==route.binding || input.environment!==route.environment) throw new Failure(403,'ScopeMismatch');
        s.consume('nonce:'+proof.keyId+':'+proof.nonce,proof.expires+31);
        return reply(200,{valid:true,expiresAt:route.expires});
      }
      fields(input,['deliveryToken','payload']);payload(input.payload,now);
      const aps=input.payload.aps,state=aps['content-state'];
      const initial=activity(s,token,route);
      admissible(initial.value,aps,now);
      const event=id();
      const outgoing={aps:{timestamp:aps.timestamp,event:aps.event,'content-state':state,...(aps.event==='end'?{'dismissal-date':aps.timestamp+60}:{'stale-date':aps.timestamp+180})}};
      try {
        const outcome=await sendAPNs(this.env,{token:route.token,environment:route.environment,exp:route.expires},{payload:outgoing,urgent:false,expiration:now+180},now,async (url,init)=>{
          policy(this.env,route.keyId,route.hostPublicKey);
          const at=Math.floor(Date.now()/1000);
          if(proof.expires<at) throw new Failure(401,'ExpiredProof');
          payload(input.payload,at);
          const active=s.get('route:'+token),client=s.get('device:'+route.keyId);
          if(!active || active.revoked || active.expires<=at || !client || client.disabled) throw new Failure(410,'InvalidSubscription');
          // JWT preparation can yield. Recheck the activity and reserve all delivery
          // quotas atomically immediately before the network attempt.
          s.transaction(()=>{
            const current=activity(s,token,active);
            admissible(current.value,aps,at);
            s.consume('nonce:'+proof.keyId+':'+proof.nonce,proof.expires+31);
            s.limit('route-delivery:'+current.slot,20,at);
            s.limit('device-delivery:'+hash(route.keyId),600,at);
            s.limit('delivery',6000,at);s.limit('daily-delivery',100000,at,86400);
            init.headers['apns-priority']=aps.event==='end' || (state.phase==='approval' && current.value.deliveredPhase!=='approval') ? '10':'5';
            s.update(current.slot,{...current.value,lastTimestamp:aps.timestamp,revision:state.revision,busyUntil:at+30,event,
              ending:current.value.ending || aps.event==='end'});
          });
          return send(url,init);
        });
        s.transaction(()=>{
          const current=s.get(initial.slot),active=s.get('route:'+token);
          if(current?.event===event && !current.closed && active && !active.revoked) {
            if(outcome.status===200) {
              s.update(initial.slot,{...current,busyUntil:0,deliveredPhase:state.phase,ended:aps.event==='end'});
              if(aps.event==='end') s.update('route:'+token,{...active,ended:true});
            }
            if(outcome.status===410) s.update('route:'+token,{...active,revoked:true});
          }
        });
        return reply(outcome.status,outcome.value);
      } finally {
        const current=s.get(initial.slot);
        if(current?.event===event && current.busyUntil!==0) s.update(initial.slot,{...current,busyUntil:0});
      }
    } catch(error) {
      return failure(error);
    }
  }
  challenge(input,purpose,now) {
    requireValue(typeof input.challenge==='string' && /^[a-f0-9]{64}$/.test(input.challenge));
    const c=this.store.get('challenge:'+input.challenge);
    if(!c || c.expires<=now || c.keyId!==input.keyId || c.purpose!==purpose) throw new Failure(401,'InvalidChallenge');
  }
  subscribe(keyId,v,now) {
    fields(v,['grant','activityId','pushToken','tokenVersion']);
    requireValue(typeof v.activityId==='string' && /^[a-zA-Z0-9-]{1,128}$/.test(v.activityId));
    requireValue(typeof v.pushToken==='string' && /^[a-f0-9]{64,512}$/.test(v.pushToken) && v.pushToken.length%2===0);
    requireValue(Number.isSafeInteger(v.tokenVersion) && v.tokenVersion>0);
    policy(this.env,keyId);
    const g=v.grant;fields(g,['hostPublicKey','keyId','binding','environment','hostId','signature','activityId','tokenVersion','tokenDigest']);
    requireValue(g.keyId===keyId && /^[a-f0-9]{64}$/.test(g.binding) && /^[a-f0-9]{64}$/.test(g.hostId));
    requireValue(g.activityId===v.activityId && g.tokenVersion===v.tokenVersion && g.tokenDigest===hash(v.pushToken));
    // The public production service never accepts development/sandbox subscriptions.
    requireValue(g.environment==='production','ProductionRequired');
    policy(this.env,keyId,g.hostPublicKey);
    checkSignature(g.hostPublicKey,g.signature,grantBase(g));
    const s=this.store, slot='slot:'+hash(keyId+':'+v.activityId), previous=s.get(slot);
    if(previous?.closed || (previous && (previous.host!==g.hostPublicKey || previous.version>v.tokenVersion))) throw new Failure(409,'SubscriptionConflict');
    const prior=previous && s.get('route:'+previous.route);
    const aggregate=prior ? activity(s,previous.route,prior).value : previous;
    if(aggregate?.ended || aggregate?.ending) throw new Failure(409,'SubscriptionConflict');
    if(aggregate?.busyUntil>now) throw new Failure(429,'DeliveryInProgress',aggregate.busyUntil-now,'subscription');
    if(prior && previous.version===v.tokenVersion) {
      if(prior.binding!==g.binding || prior.token!==v.pushToken || prior.revoked || prior.ended) throw new Failure(409,'SubscriptionConflict');
      return {deliveryToken:previous.route};
    }
    s.limit('register:'+hash(keyId),30,now);
    // Also count using a per-device membership index, so hashes do not hide active slots.
    if(!previous && s.count('member:'+hash(keyId)+':')>=256) throw new Failure(429,'SubscriptionLimit');
    const hosts=activeHosts(s,keyId,now);
    if(!hosts.includes(g.hostPublicKey) && hosts.length>=16) throw new Failure(429,'HostLimit');
    const route=id()+id(),expires=aggregate?.expires ?? now+lifetime;
    if(prior) s.update('route:'+previous.route,{...prior,revoked:true});
    s.put('route:'+route,{slot,keyId,hostPublicKey:g.hostPublicKey,binding:g.binding,environment:g.environment,token:v.pushToken,expires,revoked:false,ended:false},expires);
    s.put(slot,{lastTimestamp:0,revision:0,ended:false,busyUntil:0,...aggregate,route,host:g.hostPublicKey,version:v.tokenVersion,closed:false,expires},expires);
    s.put('member:'+hash(keyId)+':'+hash(v.activityId),true,expires);
    return {deliveryToken:route};
  }
  revoke(keyId,v,now) {
    this.store.limit('revoke:'+hash(keyId),60,now);
    fields(v,['activityId']);requireValue(typeof v.activityId==='string' && /^[a-zA-Z0-9-]{1,128}$/.test(v.activityId));
    const s=this.store,slot='slot:'+hash(keyId+':'+v.activityId),previous=s.get(slot);
    if(previous) { const r=s.get('route:'+previous.route);if(r)s.update('route:'+previous.route,{...r,revoked:true}); }
    s.put(slot,{...previous,closed:true},now+lifetime);
    return {revoked:true};
  }
}
