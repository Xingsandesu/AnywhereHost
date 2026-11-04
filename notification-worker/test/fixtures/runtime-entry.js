// Local workerd harness only. Never referenced by the production Wrangler entry.
import worker,{RelayAuthority} from '../../src/index.js';
export class RuntimeAuthority extends RelayAuthority {
  constructor(ctx,env) {super(ctx,env);
    this.metrics={read:0,written:0};
    this.authority.store.sql={exec:(...args)=>{
      const cursor=ctx.storage.sql.exec(...args),rows=cursor.toArray();
      this.metrics.read+=cursor.rowsRead;this.metrics.written+=cursor.rowsWritten;
      return {toArray:()=>rows,one:()=>{if(rows.length!==1)throw Error('Expected one row');return rows[0];}};
    }};
    const network=JSON.parse(env.TEST_NETWORK);this.network.store.put('key:'+network.keyId,{publicKey:network.publicKey,role:'client',counter:0});
    const device=JSON.parse(env.TEST_DEVICE);if(!this.authority.store.get('device:'+device.keyId)) this.authority.store.put('device:'+device.keyId,{publicKey:device.publicKey,counter:0,disabled:false});}
  async fetch(request) {
    if(new URL(request.url).pathname==='/test/network-challenge') {const v=await request.json();this.network.store.put('challenge:'+v.challenge,{keyId:v.keyId,role:v.role,purpose:'lease'},Math.floor(Date.now()/1000)+120);return Response.json({});}
    if(new URL(request.url).pathname.startsWith('/v3/network/')) {const r=await this.network.fetch(request);await this.schedule();return r;}
    if(new URL(request.url).pathname==='/test/alarm') return Response.json({at:await this.ctx.storage.getAlarm()});
    if(new URL(request.url).pathname==='/test/metrics') return Response.json(this.metrics);
    const response=await this.authority.fetch(request,async(url,init)=>{
    if(url!=='https://api.push.apple.com/3/device/'+'d'.repeat(64) || init.headers['apns-topic']!=='com.jintongshu.AnyWhereCoding.push-type.liveactivity' || init.headers['apns-push-type']!=='liveactivity') throw new Error('Unexpected APNs destination');
    return new Response(null,{status:200});
  });await this.schedule();return response;}
}
export default {fetch(request,env) {
  const url=new URL(request.url);url.protocol='https:';url.host='connect.jintongshu.com';url.port='';
  if(url.pathname==='/v2/status') return worker.fetch(new Request(url,request),{
    ...env,IP_LIMIT:{limit:async()=>({success:true})},
    AUTHORITY:{getByName(){throw Error('Status must not access Durable Objects');}}
  });
  return env.AUTHORITY.getByName('runtime-test').fetch(new Request(url,request));
}};
