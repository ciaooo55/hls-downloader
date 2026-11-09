import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { readFileSync } from 'node:fs';

// web-ext 的 Android 调试工具间接使用 forge；验证上游修复及依赖审计。
const requireExtension = createRequire(new URL('../../extension/package.json', import.meta.url));
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

// deepmerge 尚无已发布修复；验证锁定补丁保持数据属性，不调用 __proto__ setter。
const requireLinter = createRequire(requireWebExt.resolve('addons-linter'));
const merge = requireLinter('deepmerge');
const poisoned = () => JSON.parse('{"__proto__":{"hlsPoisoned":true}}');
for (const [target, source] of [
  [poisoned(), {}],
  [Object.create(null), poisoned()],
  [{ nested: poisoned() }, {}],
  [{ nested: poisoned() }, { nested: poisoned() }],
]) {
  const result = merge(target, source);
  assert.equal(Object.getPrototypeOf(result), Object.prototype);
  assert.equal(result.hlsPoisoned, undefined);
  assert.equal(result.nested?.hlsPoisoned, undefined);
  assert.equal({}.hlsPoisoned, undefined);
}
const symbol = Symbol('merge');
assert.deepEqual(merge({ a: { x: 1 }, values: [1] }, { a: { y: 2 }, values: [2], [symbol]: 3 }),
  { a: { x: 1, y: 2 }, values: [1, 2], [symbol]: 3 });

const input = readFileSync(0, 'utf8').replace(/^\uFEFF/, '').trim();
const audit = JSON.parse(input);
assert.ok(audit.advisories && audit.metadata, 'Audit must return a complete advisory report');
const remaining = Object.values(audit.advisories).filter(advisory => !(
  advisory.module_name === 'deepmerge' &&
  advisory.github_advisory_id === 'GHSA-gcqx-w48m-w2h4' &&
  advisory.findings.length > 0 && advisory.findings.every(finding => finding.version === '4.3.1')
));
assert.equal(remaining.length, 0, JSON.stringify(remaining, null, 2));
console.log(JSON.stringify({ passed: true, valid_signatures: 2, malformed_signatures_rejected: 4,
  forge_version: requireAdb('node-forge/package.json').version,
  locally_patched_advisory: 'GHSA-gcqx-w48m-w2h4', prototype_poisoning_cases: 4,
  unaddressed_advisories: remaining.length }));
