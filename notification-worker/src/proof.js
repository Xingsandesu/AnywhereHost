import {createHash, createPublicKey, verify} from 'node:crypto';
import {Failure} from './validation.js';
export const ORIGIN = 'https://connect.jintongshu.com';
export const hash = bytes => createHash('sha256').update(bytes).digest('hex');
export function base64(value, length) {
  if (typeof value !== 'string' || value.length > 32768) throw new Failure(401, 'InvalidProof');
  const b = Buffer.from(value, 'base64');
  if (b.toString('base64') !== value || (length !== undefined && b.length !== length)) throw new Failure(401, 'InvalidProof');
  return b;
}
export function publicKey(value) {
  const raw = base64(value, 65);
  if (raw[0] !== 4) throw new Failure(401, 'InvalidProof');
  return createPublicKey({key:{kty:'EC',crv:'P-256',x:raw.subarray(1,33).toString('base64url'),y:raw.subarray(33).toString('base64url')},format:'jwk'});
}
export function checkSignature(key, signature, data) {
  // workerd's one-shot verify rejects a nested PublicKeyObject; SPKI PEM works in both runtimes.
  try {
    if (verify('sha256', Buffer.from(data), {key:publicKey(key).export({type:'spki',format:'pem'}),dsaEncoding:'ieee-p1363'},base64(signature,64))) return;
  } catch {}
  throw new Failure(401,'InvalidProof');
}
export function grantBase(g) {
  return ['Anywhere-Relay-Grant-v2',ORIGIN,g.hostPublicKey,g.keyId,g.binding,g.environment,g.hostId,g.activityId,String(g.tokenVersion),g.tokenDigest].join('\n');
}
// A deliberately fixed RFC 9421 profile. No algorithm negotiation or unsigned routing fields.
export function verifyHost(request, raw, key, now) {
  if (request.headers.has('authorization')) throw new Failure(401,'InvalidCredential');
  const input = request.headers.get('signature-input') ?? '';
  const match = /^host=\("@method" "@target-uri" "content-digest"\);created=(\d{10});expires=(\d{10});nonce="([a-f0-9]{32})";keyid="([a-f0-9]{64})";alg="ecdsa-p256-sha256";tag="anywhere-relay-v2"$/.exec(input);
  if (!match) throw new Failure(401,'InvalidProof');
  const [,created,expires,nonce,keyId] = match;
  if (+created > now+30 || +created < now-90 || +expires <= now || +expires !== +created+90 || keyId !== hash(base64(key,65))) throw new Failure(401,'ExpiredProof');
  const digest = 'sha-256=:' + createHash('sha256').update(raw).digest('base64') + ':';
  if (request.headers.get('content-digest') !== digest) throw new Failure(401,'InvalidProof');
  const signature = /^host=:([A-Za-z0-9+/]+={0,2}):$/.exec(request.headers.get('signature') ?? '');
  if (!signature) throw new Failure(401,'InvalidProof');
  const params = input.slice(5);
  const data = [`"@method": POST`,`"@target-uri": ${request.url}`,`"content-digest": ${digest}`,`"@signature-params": ${params}`].join('\n');
  checkSignature(key,signature[1],data);
  return {nonce,keyId,expires:+expires};
}
