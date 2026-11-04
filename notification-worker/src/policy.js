import {Failure} from './validation.js';
import {hash,base64} from './proof.js';
// Operational emergency controls live in a Worker Secret, never in public HTTP APIs.
export function policy(env,keyId,hostPublicKey) {
  let p;
  try {
    p=JSON.parse(env.NOTIFICATION_POLICY ?? '{"disabled":false,"revokedDevices":[],"revokedHosts":[]}');
    if(typeof p.disabled!=='boolean' || Object.keys(p).sort().join(',')!=='disabled,revokedDevices,revokedHosts' ||
      ![p.revokedDevices,p.revokedHosts].every(list=>Array.isArray(list)&&list.length<=10000&&list.every(id=>typeof id==='string'&&/^[a-f0-9]{64}$/.test(id)))) throw new Error();
  } catch {throw new Failure(503,'PolicyUnavailable');}
  if(p.disabled) throw new Failure(503,'NotificationsDisabled');
  if(keyId && p.revokedDevices.includes(hash(keyId))) throw new Failure(403,'DeviceRevoked');
  if(hostPublicKey && p.revokedHosts.includes(hash(base64(hostPublicKey,65)))) throw new Failure(403,'HostRevoked');
}
