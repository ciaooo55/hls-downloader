import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { existsSync, readFileSync, realpathSync } from 'node:fs';
import { join } from 'node:path';

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
// Check the package installed for each reported consumer. The build uses two
// major versions, and a single patched copy cannot cover both dependency trees.
const checkedMatchers = new Set();
function checkedMatcherFinding(finding) {
  if (!['2.3.2', '4.0.7'].includes(finding.version) || !finding.paths?.length) return false;
  for (const path of finding.paths) {
    const [root, ...names] = path.split('>');
    assert.equal(root, '.');
    assert.equal(names.at(-1), 'picomatch');
    let consumer = requireExtension;
    let manifest;
    for (const name of names) {
      assert.match(name, /^(?:@[\w.-]+\/)?[\w.-]+$/);
      manifest = consumer.resolve.paths(name).map(dir => join(dir, name, 'package.json')).find(existsSync);
      assert.ok(manifest, `Missing installed package for ${path}`);
      manifest = realpathSync(manifest);
      consumer = createRequire(manifest);
    }
    assert.equal(JSON.parse(readFileSync(manifest, 'utf8')).version, finding.version);
    if (checkedMatchers.has(manifest)) continue;
    const match = consumer('./');
    assert.equal(match('**/*.{js,ts}')('src/main.ts'), true);
    assert.equal(match('*.txt.+(test|spec)')('a.txt.testspec'), true);
    for (const pattern of ['+(file.name)', '*(file.name)']) {
      assert.throws(() => match(pattern), /Unsupported repeated extglob in the HLS build/);
    }
    checkedMatchers.add(manifest);
  }
  return true;
}
const remaining = Object.values(audit.advisories).filter(advisory => !(
  advisory.module_name === 'deepmerge' &&
  advisory.github_advisory_id === 'GHSA-gcqx-w48m-w2h4' &&
  advisory.findings.length > 0 && advisory.findings.every(finding => finding.version === '4.3.1')
) && !(
  advisory.module_name === 'picomatch' && advisory.github_advisory_id === 'GHSA-fc5q-39xc-vjpf' &&
  advisory.findings.length > 0 && advisory.findings.every(checkedMatcherFinding)
));
assert.equal(remaining.length, 0, JSON.stringify(remaining, null, 2));
console.log(JSON.stringify({ passed: true, valid_signatures: 2, malformed_signatures_rejected: 4,
  forge_version: requireAdb('node-forge/package.json').version,
  locally_patched_advisory: 'GHSA-gcqx-w48m-w2h4', prototype_poisoning_cases: 4,
  checked_matcher_installs: checkedMatchers.size, unaddressed_advisories: remaining.length }));
