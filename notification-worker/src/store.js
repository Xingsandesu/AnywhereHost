import {Failure} from './validation.js';
export class Store {
  constructor(storage) {
    this.storage=storage; this.sql=storage.sql; this.reservations=new Map();
    this.sql.exec('CREATE TABLE IF NOT EXISTS state (id TEXT PRIMARY KEY, value TEXT NOT NULL, expires INTEGER NOT NULL)');
    this.sql.exec('CREATE INDEX IF NOT EXISTS expiry ON state(expires)');
    this.sql.exec("CREATE INDEX IF NOT EXISTS route_device ON state(json_extract(value,'$.keyId')) WHERE id>='route:' AND id<'route;'");
    this.nextExpiry=this.expiry();
  }
  get(id,now=Math.floor(Date.now()/1000)) { const row=this.sql.exec('SELECT value FROM state WHERE id=? AND expires>?',id,now).toArray()[0]; return row ? JSON.parse(row.value):null; }
  put(id,value,expires=253402300799) {
    // Updating an unindexed value must not delete/reinsert the row and its indexes.
    if(expires<253402300799) this.nextExpiry=Math.min(this.nextExpiry,expires);
    this.sql.exec(`INSERT INTO state VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET
      value=excluded.value,expires=excluded.expires
      WHERE state.value!=excluded.value OR state.expires!=excluded.expires`,id,JSON.stringify(value),expires);
  }
  delete(id) { this.sql.exec('DELETE FROM state WHERE id=?',id); }
  update(id,value) { this.sql.exec('UPDATE state SET value=? WHERE id=? AND value!=?',JSON.stringify(value),id,JSON.stringify(value)); }
  expiry() { return this.sql.exec('SELECT MIN(expires) AS n FROM state WHERE expires<253402300799').one().n ?? Infinity; }
  clean(now) { this.sql.exec('DELETE FROM state WHERE expires<=?',now); this.nextExpiry=this.expiry(); }
  count(prefix) { return this.sql.exec('SELECT count(*) AS n FROM state WHERE id>=? AND id<? AND expires>?',prefix,prefix+'~',Math.floor(Date.now()/1000)).one().n; }
  transaction(fn) {
    const reservations=new Map([...this.reservations].map(([k,v])=>[k,{...v}]));
    const nextExpiry=this.nextExpiry;
    try { return this.storage.transactionSync(fn); }
    catch(error) { this.reservations=reservations;this.nextExpiry=nextExpiry;throw error; }
  }
  limit(key,maximum,now,period=60) {
    const window=Math.floor(now/period),end=(window+1)*period;
    const id=`quota:${key}`,cacheKey=`${id}:${window}`;
    const reserved=this.reservations.get(cacheKey);
    if(reserved?.remaining>0) { reserved.remaining--; return; }
    const previous=this.get(id,now);
    const count=previous?.window===window ? previous.count : 0;
    if (count>=maximum) throw new Failure(429,'RateLimited',end-now,
      key.startsWith('route-delivery:') || key.startsWith('device-delivery:') ? 'subscription' : 'service');
    // Long-lived quotas must persist each use: ordinary idle hibernation can
    // discard a reservation after every sparse request for the entire day.
    const size=period>60 ? 1 : Math.min(maximum-count,Math.max(1,Math.min(64,Math.floor(maximum/10))));
    // Reuse the same row across minute windows. Retain it to the hour boundary
    // instead of creating/deleting an indexed counter for every key every minute.
    const expires=Math.ceil(end/3600)*3600,value={window,count:count+size,expires};
    // Minute quotas reserve BEFORE use; eviction only forfeits that minute's
    // unused capacity. Long-lived quotas have no unused reservation to lose.
    if(previous?.expires===expires) this.update(id,value); else this.put(id,value,expires);
    if(size>1) {
      for(const [key,value] of this.reservations) if(value.expires<=now) this.reservations.delete(key);
      if(this.reservations.size>=4096) this.reservations.delete(this.reservations.keys().next().value);
      this.reservations.set(cacheKey,{remaining:size-1,expires:end});
    }
  }
  consume(id,expires) {
    if (this.get(id)) throw new Failure(409,'ReplayRejected');
    this.put(id,true,expires);
  }
}
