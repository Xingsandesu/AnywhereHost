import {DurableObject} from 'cloudflare:workers';
import {Authority} from './authority.js';
import {NetworkAuthority} from './network.js';
import {status} from './status.js';
export class RelayAuthority extends DurableObject {
  constructor(ctx,env) {
    super(ctx,env);this.authority=new Authority(ctx.storage,env);this.network=new NetworkAuthority(ctx.storage,env);
    ctx.blockConcurrencyWhile(async()=>{this.alarmAt=await ctx.storage.getAlarm();await this.schedule();});
  }
  async schedule() {
    // Round expiry up to a five-minute cleanup bucket. Expiry is enforced on
    // reads, independently of physical deletion. No records means no wakeups.
    const at=Math.ceil(Math.min(this.authority.store.nextExpiry,this.network.store.nextExpiry)/300)*300000;
    if(Number.isFinite(at) && (this.alarmAt===null || at<this.alarmAt)) {
      this.alarmAt=Math.max(at,Date.now()+1000);
      try { await this.ctx.storage.setAlarm(this.alarmAt); }
      catch(error) { this.alarmAt=null;throw error; }
    }
  }
  async alarm() {
    this.alarmAt=null;
    this.authority.store.clean(Math.floor(Date.now()/1000));
    this.network.store.clean(Math.floor(Date.now()/1000));
    await this.schedule();
  }
  async fetch(request) {const response=await (new URL(request.url).pathname.startsWith('/v3/network/')?this.network:this.authority).fetch(request);await this.schedule();return response;}
}
export default {
  async fetch(request,env) {
    const path=new URL(request.url).pathname;
    if(!path.startsWith('/v2/') && !path.startsWith('/v3/network/')) return Response.json({error:'ProtocolUpgradeRequired'},{status:410,headers:{'cache-control':'no-store'}});
    if(!(await env.IP_LIMIT.limit({key:request.headers.get('cf-connecting-ip') ?? 'unknown'})).success) return Response.json({error:'RateLimited',retryAfter:60},{status:429,headers:{'retry-after':'60','cache-control':'no-store'}});
    if(path==='/v2/status') return status(request,env);
    return env.AUTHORITY.getByName(path.startsWith('/v3/network/')?'network-v3':'relay-v2').fetch(request);
  }
};
