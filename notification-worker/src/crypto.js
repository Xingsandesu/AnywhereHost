const encoder = new TextEncoder();
const b64 = value => btoa(String.fromCharCode(...new Uint8Array(value))).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '');
const json64 = value => b64(encoder.encode(JSON.stringify(value)));
let providers = new Map();
export async function providerJWT(config, now) {
  // One promise per environment/key prevents concurrent requests re-signing JWTs.
  const cacheKey = config.keyId;
  let entry = providers.get(cacheKey);
  if (!entry || entry.config !== config || now < entry.at || now - entry.at >= 3000) {
    const token = (async () => {
      const der = Uint8Array.from(atob(config.privateKey.replace(/-----[^-]+-----|\s/g, '')), c => c.charCodeAt(0));
      const signingKey = await crypto.subtle.importKey('pkcs8', der, {name:'ECDSA', namedCurve:'P-256'}, false, ['sign']);
      const unsigned = json64({alg:'ES256', kid:config.keyId}) + '.' + json64({iss:config.teamId, iat:now});
      const signature = await crypto.subtle.sign({name:'ECDSA', hash:'SHA-256'}, signingKey, encoder.encode(unsigned));
      return unsigned + '.' + b64(signature);
    })();
    entry = {config, at:now, token};
    providers.set(cacheKey, entry);
    token.catch(() => { if (providers.get(cacheKey) === entry) providers.delete(cacheKey); });
  }
  return entry.token;
}
