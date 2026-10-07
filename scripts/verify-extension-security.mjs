import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFileSync } from 'node:fs';

// web-ext 的 Android 调试工具间接使用 forge；无上游修复时验证锁定的本地补丁。
const requireExtension = createRequire(new URL('../extension/package.json', import.meta.url));
const requireWebExt = createRequire(requireExtension.resolve('web-ext'));
const requireAdb = createRequire(requireWebExt.resolve('@devicefarmer/adbkit'));
const forge = requireAdb('node-forge');
const { asn1, pki, md } = forge;
const key = pki.rsa.generateKeyPair({ bits: 1024, e: 0x10001 });
const digest = md.sha256.create().update('HLS release signature regression');
const rawDigest = digest.digest().getBytes();
assert.equal(key.publicKey.verify(rawDigest, key.privateKey.sign(digest)), true);

function encodedDigest(extra, parameters = true) {
  const algorithm = [asn1.create(asn1.Class.UNIVERSAL, asn1.Type.OID, false, asn1.oidToDer(forge.oids.sha256).getBytes())];
  if (parameters) algorithm.push(asn1.create(asn1.Class.UNIVERSAL, asn1.Type.NULL, false, ''));
  algorithm.push(...extra);
  return asn1.toDer(asn1.create(asn1.Class.UNIVERSAL, asn1.Type.SEQUENCE, true, [
    asn1.create(asn1.Class.UNIVERSAL, asn1.Type.SEQUENCE, true, algorithm),
    asn1.create(asn1.Class.UNIVERSAL, asn1.Type.OCTETSTRING, false, rawDigest),
  ])).getBytes();
}
function signed(info) { return key.privateKey.sign(info, 'NONE'); }
assert.equal(key.publicKey.verify(rawDigest, signed(encodedDigest([], false))), true);
for (const extra of [
  asn1.create(asn1.Class.UNIVERSAL, asn1.Type.NULL, false, ''),
  asn1.create(asn1.Class.UNIVERSAL, asn1.Type.OCTETSTRING, false, 'garbage'),
  asn1.create(asn1.Class.UNIVERSAL, asn1.Type.SEQUENCE, true, []),
]) {
  assert.throws(() => key.publicKey.verify(rawDigest, signed(encodedDigest([extra]))), /DigestInfo/);
}
// 构造合法 DER 长度，但禁止 NULL 内藏数据。
const nullAlgorithm = asn1.fromDer(encodedDigest([], true));
nullAlgorithm.value[0].value[1].value = 'garbage';
assert.throws(() => key.publicKey.verify(rawDigest, signed(asn1.toDer(nullAlgorithm).getBytes())), /DigestInfo/);

const input = readFileSync(0, 'utf8').replace(/^\uFEFF/, '').trim();
const audit = JSON.parse(input);
assert.ok(audit.advisories && audit.metadata, 'Audit must return a complete advisory report');
const remaining = Object.values(audit.advisories).filter(advisory => {
  // 只认可本脚本刚刚验证过的、仅影响已补丁 forge 1.4.0 的上游告警。
  return !(advisory.module_name === 'node-forge' &&
    advisory.github_advisory_id === 'GHSA-86w9-cpqp-85rv' &&
    advisory.findings.length > 0 && advisory.findings.every(finding => finding.version === '1.4.0'));
});
assert.equal(remaining.length, 0, JSON.stringify(remaining, null, 2));
console.log(JSON.stringify({ passed: true, valid_signatures: 2, malformed_signatures_rejected: 4,
  locally_patched_advisory: 'GHSA-86w9-cpqp-85rv', unaddressed_advisories: remaining.length }));
