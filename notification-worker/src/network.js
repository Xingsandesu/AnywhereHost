import {randomBytes,createPublicKey,verify,sign} from 'node:crypto';
import {Store} from './store.js';
import {releaseIdentity,localReleases,developmentSessions} from './releases.js';
import {attest,assertion} from './attestation.js';
import {hash,base64} from './proof.js';
import {Failure,fields,requireValue,body} from './validation.js';
import {allowed,reply,failure} from './status.js';
const id=()=>randomBytes(32).toString('hex');
export const ISSUER='https://connect.jintongshu.com';
export const KID='admission-1';
export function settings(env) {
  let p;try {p=JSON.parse(env.NETWORK_POLICY);}catch {throw new Failure(503,'NetworkPolicyUnavailable');}
  requireValue(p && typeof p.disabled==='boolean' && Array.isArray(p.revokedKeys) && p.revokedKeys.length<=10000 && p.revokedKeys.every(k=>typeof k==='string' && /^[a-f0-9]{64}$/.test(k)) && Array.isArray(p.relays) && p.relays.length>0 && p.relays.length<=4);
  requireValue(Array.isArray(p.hostBuilds ?? []) && (p.hostBuilds ?? []).length<=256 && (p.hostBuilds ?? []).every(b=>b && /^[a-f0-9]{64}$/.test(b.codeHash) && Number.isInteger(b.build) && b.build>=51) && Array.isArray(p.revokedHostCodeHashes ?? []) && (p.revokedHostCodeHashes ?? []).length<=10000 && (p.revokedHostCodeHashes ?? []).every(h=>/^[a-f0-9]{64}$/.test(h)));
  if(p.disabled)throw new Failure(503,'NetworkDisabled');
  for(const url of p.relays) {const u=new URL(url);requireValue(u.protocol==='https:' && !u.username && !u.password && !u.search && !u.hash && u.pathname==='/');}
  return {...p,hostBuilds:[...(p.hostBuilds ?? []),...localReleases(env.HOST_RELEASES)]};
}
export class NetworkAuthority {
  constructor(storage,env) {this.store=new Store(storage);this.env=env;}
  async fetch(request) {
    try {
      const url=new URL(request.url),path=url.pathname,s=this.store,p=settings(this.env);
      if(path==='/v3/network/releases') {
        requireValue(url.origin===ISSUER && !url.search && request.method==='POST' && !request.headers.has('origin'));
        const now=Math.floor(Date.now()/1000),token=/^Bearer ([A-Za-z0-9_.-]+)$/.exec(request.headers.get('authorization') ?? '')?.[1];
        const identity=await releaseIdentity(token,now);
        const {input}=await body(request,4096);fields(input,['codeHash','build','version','sourceSha']);
        requireValue(/^[a-f0-9]{64}$/.test(input.codeHash) && Number.isInteger(input.build) && input.build>=51 && input.build<100000000 && /^\d+\.\d+\.\d+$/.test(input.version) && /^[a-f0-9]{40}$/.test(input.sourceSha));
        const builds=s.get('host-builds') ?? [];
        const previous=builds.find(b=>b.codeHash===input.codeHash);
        if(previous && (previous.build!==input.build || previous.version!==input.version))throw new Failure(409,'ReleaseConflict');
        if(!previous && builds.length>=256)throw new Failure(503,'ReleaseCapacityReached');
        s.transaction(()=>{s.consume('release-proof:'+hash(identity.jti),identity.exp);if(!previous)s.put('host-builds',[...builds,input]);});
        return reply(200,{registered:true,codeHash:input.codeHash});
      }
      allowed(request);
      const hostBuilds=[...(p.hostBuilds ?? []),...(s.get('host-builds') ?? [])].filter((b,i,all)=>all.findIndex(x=>x.codeHash===b.codeHash)===i && !(p.revokedHostCodeHashes ?? []).includes(b.codeHash));
      const {input}=await body(request,32768),now=Math.floor(Date.now()/1000);
      s.limit('network',3000,now);
      if(path==='/v3/network/challenges') {
        fields(input,['keyId','role','purpose']);base64(input.keyId,32);
        requireValue(['host','client'].includes(input.role) && ['attest','lease'].includes(input.purpose));
        s.limit('challenge:'+hash(input.keyId),20,now);
        const known=s.get('key:'+input.keyId);
        if(known && known.role!==input.role)throw new Failure(403,'RoleMismatch');
        if(input.purpose==='attest' && known)return reply(200,{attested:true});
        const challenge=id();s.put('challenge:'+challenge,{...input,expires:now+120},now+120);
        return reply(200,{challenge});
      }
      if(path==='/v3/network/attestations') {
        fields(input,['keyId','role','challenge','attestation']);
        s.limit('attestation-source:'+hash(request.headers.get('cf-connecting-ip')??'unknown'),6,now);s.limit('attestation',60,now);
        this.challenge(input,'attest',now);
        if(s.get('key:'+input.keyId))throw new Failure(409,'AlreadyAttested');
        if(s.count('key:')>=100000)throw new Failure(503,'EnrollmentCapacityReached');
        const key=attest(input.attestation,input.keyId,input.challenge,now,input.role,51,hostBuilds);
        s.transaction(()=>{s.limit('daily-enrollment',1000,now,86400);s.put('key:'+input.keyId,{...key,role:input.role,minimumBuild:51,counter:0});s.delete('challenge:'+input.challenge);});
        return reply(200,{registered:true});
      }
      if(path!=='/v3/network/leases')throw new Failure(404,'NotFound');
      fields(input,['keyId','role','challenge','data','assertion']);this.challenge(input,'lease',now);
      const key=s.get('key:'+input.keyId);
      if(!key || key.role!==input.role || p.revokedKeys.includes(hash(input.keyId)))throw new Failure(403,'AppNotAuthorized');
      const bytes=base64(input.data),message=Buffer.concat([Buffer.from(`Anywhere-Network-v3\n${ISSUER}${path}\n${input.challenge}\n`),bytes]);
      let data;try{data=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(bytes));}catch{throw new Failure(400,'InvalidJSON');}
      fields(data,['endpointId','signature']);requireValue(typeof data.endpointId==='string' && /^[a-f0-9]{64}$/.test(data.endpointId));
      const developmentBuild=input.role==='host'?developmentSessions(this.env.HOST_DEVELOPMENT_SESSIONS,now).find(b=>b.keyHash===hash(input.keyId) && b.endpointId===data.endpointId && !(p.revokedHostCodeHashes ?? []).includes(b.codeHash)):undefined;
      const count=assertion(input.assertion,message,{...key,hostBuilds,developmentBuild});
      const publicKey=createPublicKey({key:{kty:'OKP',crv:'Ed25519',x:Buffer.from(data.endpointId,'hex').toString('base64url')},format:'jwk'});
      const proof=`Anywhere-Network-Endpoint-v3\n${ISSUER}\n${input.keyId}\n${input.role}\n${input.challenge}\n${data.endpointId}`;
      if(!verify(null,Buffer.from(proof),publicKey,base64(data.signature,64)))throw new Failure(401,'InvalidEndpointProof');
      const member='member:'+hash(input.keyId)+':'+data.endpointId;
      if(!s.get(member) && s.count('member:'+hash(input.keyId)+':')>=32)throw new Failure(429,'EndpointLimit');
      // The relay and both peers verify this certificate against the endpoint authenticated by iroh.
      const claims={iss:ISSUER,aud:'anywhere-network-v3',sub:data.endpointId,role:key.role,iat:now,exp:Math.min(now+900,developmentBuild?.expiresAt ?? Infinity),jti:id(),relays:p.relays};
      const encoded=[Buffer.from(JSON.stringify({alg:'EdDSA',typ:'JWT',kid:KID})).toString('base64url'),Buffer.from(JSON.stringify(claims)).toString('base64url')].join('.');
      const token=encoded+'.'+sign(null,Buffer.from(encoded),this.env.NETWORK_SIGNING_KEY).toString('base64url');
      s.transaction(()=>{this.challenge(input,'lease',now);const current=s.get('key:'+input.keyId);if(count<=current.counter)throw new Failure(409,'ReplayRejected');s.update('key:'+input.keyId,{...current,counter:count});s.put(member,true,now+900);s.delete('challenge:'+input.challenge);});
      return reply(200,{token,expiresAt:claims.exp,relays:p.relays});
    }catch(error){return failure(error);}
  }
  challenge(input,purpose,now) {
    requireValue(typeof input.challenge==='string' && /^[a-f0-9]{64}$/.test(input.challenge));
    const c=this.store.get('challenge:'+input.challenge,now);
    if(!c || c.keyId!==input.keyId || c.role!==input.role || c.purpose!==purpose)throw new Failure(401,'InvalidChallenge');
  }
}
