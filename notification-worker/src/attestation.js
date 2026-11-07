import {X509Certificate} from 'node:crypto';
import cbor from 'cbor';
import * as asn1js from 'asn1js';
import {Certificate} from 'pkijs';
import {verifyAttestation, verifyAssertion} from 'node-app-attest';
import {base64} from './proof.js';
import {Failure} from './validation.js';
const apps = {client:{bundleIdentifier:'com.jintongshu.AnyWhereCoding',teamIdentifier:'775Y7KV7KU'},host:{bundleIdentifier:'com.jintongshu.AnywhereHost',teamIdentifier:'775Y7KV7KU'}};
export function decode(value) {
  const values = cbor.decodeAllSync(value, {max_depth:16,preventDuplicateKeys:true});
  if (values.length !== 1) throw new Error('InvalidCBOR');
  return values[0];
}
export function extensions(authData, offset, role='client', minimumBuild=1, hostBuilds=[], developmentBuild) {
  // iOS 27 includes this signed CBOR map even when the ED bit is unset.
  // Apple’s validation-guide fixture encodes the category as UInt32 little-endian bytes.
  if(authData.length<=offset) throw new Error('MissingClientExtensions');
  const values=decode(authData.subarray(offset));
  if(!values || typeof values!=='object' || Array.isArray(values) || values instanceof Map || Buffer.isBuffer(values)) throw new Error('InvalidClientExtensions');
  if((Object.hasOwn(values,'apple_validation_category_01') && Object.hasOwn(values,'validationCategory')) || (Object.hasOwn(values,'apple_bundle_version_01') && Object.hasOwn(values,'bundleVersion'))) throw new Error('AmbiguousClientExtensions');
  const rawCategory=values.apple_validation_category_01 ?? values.validationCategory;
  const category=Buffer.isBuffer(rawCategory) && rawCategory.length===4 ? rawCategory.readUInt32LE() : rawCategory;
  const version=values.apple_bundle_version_01 ?? values.bundleVersion;
  if(role==='host') {
    const rawType=values.apple_cd_hash_type_01;
    const type=Buffer.isBuffer(rawType) && rawType.length===1 ? rawType[0] : rawType;
    const code=values.apple_cd_hash_hash_01;
    if(type!==2 || !Buffer.isBuffer(code) || code.length!==32)throw new Error('UnsupportedHostCode');
    const candidates=category===6?hostBuilds:(category===3 && developmentBuild?[developmentBuild]:[]);
    const matches=candidates.filter(b=>b.build>=minimumBuild && b.codeHash===code.toString('hex'));
    if(matches.length!==1)throw new Error('UnapprovedHostBuild');
    if(version!==undefined && version!==String(matches[0].build))throw new Error('HostBuildMismatch');
  } else if(![2,4].includes(category) || typeof version!=='string' || !/^[0-9]{1,8}$/.test(version) || Number(version)<minimumBuild)throw new Error('UnsupportedClient');
}
export function attest(value, keyId, challenge, now, role='client', minimumBuild=1, hostBuilds=[]) {
  try {
    base64(keyId,32);
    const bytes = base64(value);
    const decoded = decode(bytes);
    if (decoded?.attStmt?.x5c?.length !== 2 || !Buffer.isBuffer(decoded.authData) || decoded.authData.length < 87) throw new Error();
    const [leaf, intermediate] = decoded.attStmt.x5c.map(b=>new X509Certificate(b));
    for (const cert of [leaf,intermediate]) {
      if (Date.parse(cert.validFrom) > now*1000 || Date.parse(cert.validTo) <= now*1000) throw new Error();
    }
    if (leaf.ca || !intermediate.ca || !leaf.checkIssued(intermediate) || leaf.publicKey.asymmetricKeyDetails?.namedCurve !== 'prime256v1') throw new Error();
    if (decoded.authData.readUInt16BE(53) !== 32 || (decoded.authData[32] & 0x40) === 0) throw new Error();
    const cose=cbor.decodeFirstSync(decoded.authData.subarray(87),{extendedResults:true,preventDuplicateKeys:true,max_depth:4});
    extensions(decoded.authData,87+cose.length,role,minimumBuild,hostBuilds);
    if(role==='host') {
      const cert=new Certificate({schema:asn1js.fromBER(leaf.raw).result});
      const matches=cert.extensions?.filter(e=>e.extnID==='1.2.840.113635.100.8.6') ?? [];
      if(matches.length!==1) throw new Error('MissingMacPolicy');
      const decoded=asn1js.fromBER(matches[0].extnValue.valueBlock.valueHexView);
      const octets=[];
      const visit=node=>{if(node.idBlock?.tagClass===1&&node.idBlock?.tagNumber===4)octets.push(Buffer.from(node.valueBlock.valueHexView));else for(const child of node.valueBlock?.value??[])visit(child);};
      if(decoded.offset===-1)throw new Error('InvalidMacPolicy');visit(decoded.result);
      if(octets.length!==1 || octets[0].toString('base64')!=='MEAMAjExMDowCQwCb2uhAwEB/zAJDAJvYaEDAQH/MAsMBG9kZWyhAwEB/zAVDARvc2duoAYMBHJzZWMwBaYDAgEB')throw new Error('UnsafeMacPolicy');
    }
    const result = verifyAttestation({attestation:bytes,keyId,challenge,...apps[role],allowDevelopmentEnvironment:false});
    return {publicKey:result.publicKey,receipt:result.receipt.toString('base64')};
  } catch { throw new Failure(401,'InvalidAttestation'); }
}
export function assertion(value, data, client) {
  try {
    const bytes = base64(value);
    const decoded = decode(bytes);
    if (!Buffer.isBuffer(decoded.signature) || !Buffer.isBuffer(decoded.authenticatorData) || decoded.authenticatorData.length < 37) throw new Error();
    extensions(decoded.authenticatorData,37,client.role,client.minimumBuild ?? 1,client.hostBuilds ?? [],client.developmentBuild);
    return verifyAssertion({assertion:bytes,payload:data,publicKey:client.publicKey,signCount:client.counter,...apps[client.role ?? 'client']}).signCount;
  } catch { throw new Failure(401,'InvalidAssertion'); }
}
