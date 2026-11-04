import {providerJWT} from './crypto.js';
import {Failure,requireValue,fields} from './validation.js';
const reply=(status,value)=>({status,value});
let configCache;
function providers(env) {
  if (configCache?.raw !== env.APNS_CONFIG) {
    let config;
    try { config = JSON.parse(env.APNS_CONFIG); } catch { throw new Failure(503, 'ProviderNotConfigured'); }
    requireValue(config && Object.keys(config).every(k => ['sandbox','production'].includes(k)));
    for (const c of Object.values(config)) {
      fields(c, ['teamId','keyId','privateKey']);
      requireValue(/^[A-Z0-9]{10}$/.test(c.teamId) && /^[A-Z0-9]{10}$/.test(c.keyId) && typeof c.privateKey === 'string');
    }
    configCache = {raw:env.APNS_CONFIG, config};
  }
  return configCache.config;
}
export async function sendAPNs(env,t,input,now,send=fetch) {
    const c = providers(env)[t.environment];
    if (!c) throw new Failure(503, 'EnvironmentNotConfigured');
    const jwt = await providerJWT(c, now);
    const origin = t.environment === 'sandbox' ? 'api.sandbox.push.apple.com' : 'api.push.apple.com';
    let response;
    try { response = await send(`https://${origin}/3/device/${t.token}`, {
      method:'POST',redirect:'manual',signal:AbortSignal.timeout(15000),
      headers:{authorization:`bearer ${jwt}`,'apns-topic':'com.jintongshu.AnyWhereCoding.push-type.liveactivity',
        'apns-push-type':'liveactivity','apns-priority':input.urgent ? '10':'5','apns-expiration':String(Math.min(input.expiration,t.exp)), 'content-type':'application/json'},
      body:JSON.stringify(input.payload)
    }); } catch (error) {
      if(error instanceof Failure) throw error;
      throw new Failure(503, error?.name === "TimeoutError" ? "APNsTimeout" : "APNsTransportUnavailable");
    }
    if (response.status === 200) { await response.body?.cancel(); return reply(200,{accepted:true}); }
    // Apple errors are deliberately reduced to stable codes; no payload/token logging.
    let reason;
    if (response.body) {
      const reader = response.body.getReader(); let size = 0; const chunks = [];
      try {
        while (true) {
          const {done,value} = await reader.read(); if (done) break;
          size += value.length; if (size > 4096) { await reader.cancel(); throw new Failure(503,'APNsUnavailable'); }
          chunks.push(value);
        }
      } finally { reader.releaseLock(); }
      const bytes = new Uint8Array(size); let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk,offset); offset += chunk.length; }
      try { reason = JSON.parse(new TextDecoder().decode(bytes)).reason; } catch {}
    }
    if (response.status === 410 || ['BadDeviceToken','DeviceTokenNotForTopic','Unregistered'].includes(reason)) return reply(410,{error:'InvalidDeviceToken'});
    const delay = response.status === 403 ? 900 : Math.min(3600,Math.max(5,Number(response.headers.get('retry-after')) || 5));
    // TooManyRequests applies to this device token. Provider-token throttling
    // and unknown failures must still back off the shared service transport.
    if (response.status === 429 && reason === 'TooManyRequests') return reply(429,{error:'APNsRateLimited',retryAfter:delay,retryScope:'subscription'});
    if (response.status === 429 || response.status >= 500 || response.status === 403) return reply(503,{error:'APNsUnavailable',retryAfter:delay});
    return reply(422,{error:'APNsRejected'});
}
export async function providerStatus(env,now) {
 const config=providers(env);
 await Promise.all(Object.values(config).map(c=>providerJWT(c,now)));
 return {enabled:!!config.production,environments:Object.keys(config).filter(e=>e==='production'),transport:'relay-v2'};
}
