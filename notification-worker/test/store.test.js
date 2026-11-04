import {test} from 'node:test';
import assert from 'node:assert/strict';
import {Store} from '../src/store.js';
import {storage} from './helpers/relay.js';

test('quota reservations preserve hard limits through eviction and window rollover',()=>{
  const disk=storage();let s=new Store(disk);
  for(let i=0;i<600;i++) s.limit('host',600,1000);
  assert.throws(()=>s.limit('host',600,1000),/RateLimited/);
  s=new Store(disk);assert.throws(()=>s.limit('host',600,1000),/RateLimited/);
  for(let i=0;i<600;i++) s.limit('host',600,1020);
  assert.throws(()=>s.limit('host',600,1020),/RateLimited/);
  const disk2=storage();s=new Store(disk2);s.limit('host',600,1000);
  s=new Store(disk2);
  // Unused reservation is forfeited on restart, never made available twice.
  for(let i=0;i<540;i++) s.limit('host',600,1000);
  assert.throws(()=>s.limit('host',600,1000),/RateLimited/);
});
test('rolled-back reservations cannot grant unpersisted quota',()=>{
  const s=new Store(storage());
  assert.throws(()=>s.transaction(()=>{s.limit('host',600,1000);throw Error('rollback');}));
  assert.equal(s.reservations.size,0);
  assert.equal(s.nextExpiry,Infinity);
  for(let i=0;i<600;i++) s.limit('host',600,1000);
  assert.throws(()=>s.limit('host',600,1000),/RateLimited/);
});
test('daily quotas count exact usage through repeated reconstruction and rollover',()=>{
  const at=86400+60;
  for(const [key,maximum] of [['daily-delivery',100000],['daily-attestations',1000]]) {
    const disk=storage();let s=new Store(disk);
    for(let i=0;i<maximum;i++) {
      if(i%31===0) s=new Store(disk);
      s.limit(key,maximum,at,86400);
    }
    s=new Store(disk);
    assert.equal(s.get(`quota:${key}`,at).count,maximum);
    assert.throws(()=>s.limit(key,maximum,at,86400),/RateLimited/);
    s.limit(key,maximum,172800,86400);
    assert.equal(new Store(disk).get(`quota:${key}`,172800).count,1);
  }
});
test('daily quota rollback preserves the persisted hard limit',()=>{
  const disk=storage(),at=86400+60,key='daily-delivery',maximum=100000;
  let s=new Store(disk);
  s.put('quota:daily-delivery',{window:1,count:maximum-2,expires:172800},172800);
  s.limit(key,maximum,at,86400);
  s=new Store(disk);
  assert.throws(()=>s.transaction(()=>{s.limit(key,maximum,at,86400);throw Error('rollback');}),/rollback/);
  s=new Store(disk);
  s.limit(key,maximum,at,86400);
  assert.throws(()=>new Store(disk).limit(key,maximum,at,86400),/RateLimited/);
});
test('expired slots and memberships are invisible before physical cleanup',()=>{
  const s=new Store(storage()),now=Math.floor(Date.now()/1000);
  s.put('member:device:a',true,now-1);s.put('slot:a',{closed:true},now-1);
  s.put('member:device:b',true,now+120);
  assert.equal(s.get('slot:a'),null);assert.equal(s.count('member:device:'),1);
  s.clean(now);assert.equal(s.nextExpiry,now+120);
  s.clean(now+120);assert.equal(s.nextExpiry,Infinity);
});

test('quota rows are reused across minute windows',()=>{
  const disk=storage(),s=new Store(disk),at=Math.floor(Date.now()/3600000)*3600+600;
  s.put('quota:host',{window:Math.floor(at/60),count:599,expires:at+60},at+60);
  s.limit('host',600,at);
  assert.throws(()=>s.limit('host',600,at),/RateLimited/);
  const restarted=new Store(disk);
  assert.throws(()=>restarted.limit('host',600,at),/RateLimited/);
  for(let minute=1;minute<10;minute++) restarted.limit('host',600,at+minute*60);
  assert.equal(disk.sql.exec("SELECT count(*) AS n FROM state WHERE id LIKE 'quota:host%'").one().n,1);
});

test('a rejected transaction restores already reserved balances',()=>{
 const s=new Store(storage());s.limit('host',600,1000);
 assert.throws(()=>s.transaction(()=>{s.limit('host',600,1000);throw Error('rollback');}));
 for(let i=1;i<600;i++) s.limit('host',600,1000);
 assert.throws(()=>s.limit('host',600,1000),/RateLimited/);
});
