import {Failure} from './validation.js';

// The slot is the activity's coordination unit; delivery tokens are replaceable routes.
export function activity(store, token, route) {
  const slot=route.slot, value=slot && store.get(slot);
  if(!value || value.closed || value.route!==token) throw new Failure(410,'InvalidSubscription');
  return {slot,value};
}

export function admissible(value, aps, now) {
  if(value.ended || (value.ending && aps.event!=='end') || aps.timestamp<=value.lastTimestamp || aps['content-state'].revision<value.revision)
    throw new Failure(409,'StaleEvent');
  if(value.busyUntil>now) throw new Failure(429,'DeliveryInProgress',value.busyUntil-now,'subscription');
}

export function activeHosts(store, keyId, now) {
  return store.sql.exec(`SELECT DISTINCT json_extract(value,'$.hostPublicKey') AS host FROM state
    WHERE id>='route:' AND id<'route;' AND json_extract(value,'$.keyId')=? AND expires>?
    AND json_extract(value,'$.revoked')=0 AND json_extract(value,'$.ended')=0`,keyId,now).toArray().map(r=>r.host);
}
