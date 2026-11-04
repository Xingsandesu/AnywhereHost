export class Failure extends Error {
  constructor(status, code, retryAfter, retryScope) { super(code); this.status = status; this.retryAfter = retryAfter; this.retryScope = retryScope; }
}
export function requireValue(condition, code = 'InvalidRequest') { if (!condition) throw new Failure(400, code); }
export function fields(value, names) {
  requireValue(value && typeof value === 'object' && !Array.isArray(value));
  requireValue(Object.keys(value).length === names.length && names.every(n => Object.hasOwn(value, n)));
}
export async function body(request, maximum = 8192) {
  requireValue(request.headers.get('content-type')?.split(';')[0] === 'application/json');
  if (Number(request.headers.get('content-length')) > maximum) throw new Failure(413, 'RequestTooLarge');
  if (!request.body) throw new Failure(400, 'InvalidRequest');
  const reader = request.body.getReader();
  const chunks = []; let length = 0;
  const deadline=setTimeout(()=>{ void reader.cancel(); },10000);
  try {
    while (true) {
      const {done, value} = await reader.read();
      if (done) break;
      length += value.length;
      if (length > maximum) { await reader.cancel(); throw new Failure(413, 'RequestTooLarge'); }
      chunks.push(value);
    }
  } finally { clearTimeout(deadline);reader.releaseLock(); }
  const bytes = new Uint8Array(length); let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
  try { return {input:JSON.parse(new TextDecoder('utf-8', {fatal:true}).decode(bytes)),raw:bytes}; }
  catch { throw new Failure(400, 'InvalidJSON'); }
}
export function payload(value, now) {
  fields(value, ['aps']);
  const aps = value.aps;
  fields(aps, ['timestamp','event','content-state']);
  requireValue(Number.isSafeInteger(aps.timestamp) && Math.abs(aps.timestamp - now) <= 120);
  requireValue(['update','end'].includes(aps.event));
  const state = aps['content-state'];
  fields(state, ['phase','startedAt','endedAt','updatedAt','completedTools','revision']);
  requireValue(['working','thinking','responding','approval','disconnected','completed','failed','stopped'].includes(state.phase));
  requireValue(['startedAt','updatedAt'].every(k => Number.isFinite(state[k]) && Math.abs(state[k]) < 1e11));
  requireValue(state.endedAt === null || (Number.isFinite(state.endedAt) && Math.abs(state.endedAt) < 1e11));
  requireValue(['completedTools','revision'].every(k => Number.isSafeInteger(state[k]) && state[k] >= 0));
  requireValue((aps.event === 'end') === ['completed','failed','stopped'].includes(state.phase));
  requireValue(new TextEncoder().encode(JSON.stringify(value)).length <= 4096);
}
