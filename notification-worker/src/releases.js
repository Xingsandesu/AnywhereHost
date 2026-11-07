import {createPublicKey,verify} from 'node:crypto';
import {Failure} from './validation.js';
const issuer='https://token.actions.githubusercontent.com';
export const audience='https://connect.jintongshu.com/v3/network/releases';
let cached;
// Operator-managed catalog for verified local releases. It is a deployment
// binding, never supplied by the app or accepted by the public release endpoint.
export function localReleases(value) {
  if(value===undefined)return [];
  try {
    if(typeof value!=='string' || value.length>65536)throw Error();
    const builds=JSON.parse(value);
    if(!Array.isArray(builds) || builds.length>256)throw Error();
    const hashes=new Set();
    for(const b of builds) {
      if(!b || Object.keys(b).length!==2 || typeof b.codeHash!=='string' || !/^[a-f0-9]{64}$/.test(b.codeHash) || !Number.isInteger(b.build) || b.build<51 || b.build>=100000000 || hashes.has(b.codeHash))throw Error();
      hashes.add(b.codeHash);
    }
    return builds;
  }catch {throw new Failure(503,'ReleaseCatalogUnavailable');}
}
export function verifyReleaseToken(token,keys,now) {
  try {
    if(typeof token!=='string' || token.length>16384)throw Error();
    const parts=token.split('.');if(parts.length!==3)throw Error();
    const h=JSON.parse(Buffer.from(parts[0],'base64url')),c=JSON.parse(Buffer.from(parts[1],'base64url'));
    if(h.alg!=='RS256' || typeof h.kid!=='string')throw Error();
    const matches=keys.filter(k=>k.kid===h.kid && k.kty==='RSA' && k.alg==='RS256' && k.use==='sig');
    if(matches.length!==1 || !verify('RSA-SHA256',Buffer.from(parts[0]+'.'+parts[1]),createPublicKey({format:'jwk',key:matches[0]}),Buffer.from(parts[2],'base64url')))throw Error();
    const workflows=['release.yml','register-host.yml'].map(name=>`Xingsandesu/AnywhereHost/.github/workflows/${name}@refs/heads/main`);
    if(c.iss!==issuer || c.aud!==audience || c.sub!=='repo:Xingsandesu@55939146/AnywhereHost@1440923483:ref:refs/heads/main' || c.repository_id!=='1440923483' || c.repository_owner_id!=='55939146' || c.ref!=='refs/heads/main' || !workflows.includes(c.workflow_ref) || !['workflow_run','workflow_dispatch'].includes(c.event_name))throw Error();
    if(!Number.isInteger(c.exp) || !Number.isInteger(c.iat) || !Number.isInteger(c.nbf) || c.exp<=now || c.iat>now+30 || c.nbf>now+30 || c.exp-c.iat>600 || c.exp<=c.iat || typeof c.jti!=='string' || c.jti.length<1 || c.jti.length>256)throw Error();
    return c;
  }catch{throw new Failure(403,'UntrustedReleaseWorkflow');}
}
export async function releaseIdentity(token,now) {
  if(typeof token!=='string' || token.length>16384)throw new Failure(403,'UntrustedReleaseWorkflow');
  if(!cached || cached.expires<=now) {
    const response=await fetch(issuer+'/.well-known/jwks',{redirect:'manual',signal:AbortSignal.timeout(10000),headers:{'User-Agent':'AnywhereHost-Admission/1.1.1'}});
    if(!response.ok)throw new Failure(503,'ReleaseIdentityUnavailable');
    const bytes=await response.arrayBuffer();if(bytes.byteLength>65536)throw new Failure(503,'ReleaseIdentityUnavailable');
    const {keys}=JSON.parse(new TextDecoder().decode(bytes));if(!Array.isArray(keys)||keys.length>16)throw new Failure(503,'ReleaseIdentityUnavailable');
    cached={keys,expires:now+300};
  }
  return verifyReleaseToken(token,cached.keys,now);
}
