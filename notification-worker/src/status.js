import {Failure,body,fields} from './validation.js';
import {ORIGIN} from './proof.js';
import {policy} from './policy.js';
import {providerStatus} from './apns.js';

export const reply=(status,value)=>Response.json(value,{status,headers:{'cache-control':'no-store','x-content-type-options':'nosniff',...(value.retryAfter?{'retry-after':String(value.retryAfter)}:{})}});
export function allowed(request) {
  const url=new URL(request.url);
  if(url.origin!==ORIGIN || url.search || request.method!=='POST' || request.headers.has('origin') || request.headers.has('authorization')) throw new Failure(403,'RequestNotAllowed');
}
export function failure(error) {
  return reply(error instanceof Failure?error.status:503,{error:error instanceof Failure?error.message:'ServiceUnavailable',...(error instanceof Failure && error.retryAfter?{retryAfter:error.retryAfter}:{}),...(error instanceof Failure && error.retryScope?{retryScope:error.retryScope}:{})});
}
// Configuration is isolate-local. A status probe must never wake or write a DO.
export async function status(request,env) {
  try {
    allowed(request);policy(env);
    fields((await body(request)).input,[]);
    return reply(200,await providerStatus(env,Math.floor(Date.now()/1000)));
  } catch(error) { return failure(error); }
}
