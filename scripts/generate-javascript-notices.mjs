import { createHash } from 'node:crypto'
import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(fileURLToPath(new URL('../', import.meta.url)))
const overridePath = path.join(repoRoot, 'scripts', 'javascript-notice-overrides.json')
const outputDirectory = path.join(repoRoot, 'src-tauri', 'resources', 'third-party', 'javascript')
const htmlPath = path.join(outputDirectory, 'THIRD-PARTY-NOTICES.html')
const manifestPath = path.join(outputDirectory, 'COMPONENTS.json')
const legalFilePattern = /^(?:licen[cs]e|notice|copying|copyright)/iu

const portablePath = value => value.split(path.sep).join('/')
const sha256 = value => createHash('sha256').update(value).digest('hex')
const normalizeText = value => value.replace(/\r\n?/gu, '\n')
const escapeHtml = value =>
  String(value)
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;')

const assert = (condition, message) => {
  if (!condition) throw new Error(message)
}

const readJson = async file => JSON.parse(await readFile(file, 'utf8'))

const resolveInsideRepo = relativePath => {
  const absolutePath = path.resolve(repoRoot, relativePath)
  assert(absolutePath.startsWith(`${repoRoot}${path.sep}`), `Notice input path escapes the repository: ${relativePath}`)
  return absolutePath
}

const readTextMaterial = async ({ absolutePath, displayName, sourceKind, sourceUrl = null, sourceRevision = null }) => {
  const bytes = await readFile(absolutePath)
  const text = normalizeText(new TextDecoder('utf-8', { fatal: true }).decode(bytes))
  return {
    displayName,
    sourceKind,
    sourceUrl,
    sourceRevision,
    sourceSha256: sha256(bytes),
    contentSha256: sha256(text),
    text,
  }
}

const readPinnedMaterial = async specification => {
  const absolutePath = resolveInsideRepo(specification.path)
  const material = await readTextMaterial({
    absolutePath,
    displayName: specification.displayName,
    sourceKind: 'reviewed-override',
    sourceUrl: specification.sourceUrl,
    sourceRevision: specification.sourceRevision,
  })
  assert(material.sourceSha256 === specification.sha256, `Pinned notice hash mismatch for ${specification.path}`)
  assert(/^[0-9a-f]{40}$/u.test(specification.sourceRevision), `Pinned notice has no immutable source revision`)
  assert(/^https:\/\/github\.com\//u.test(specification.sourceUrl), `Pinned notice has no canonical source URL`)
  assert(specification.sourceUrl.includes(specification.sourceRevision), `Pinned notice URL does not bind its revision`)
  assert(/^[0-9a-f]{64}$/u.test(specification.upstreamRawSha256), `Pinned notice has no upstream raw hash`)
  return material
}

const packageRootFromModule = moduleId => {
  const normalized = moduleId.replaceAll('\\', '/').replace(/^\0/u, '').split('?')[0]
  const marker = '/node_modules/'
  const markerIndex = normalized.lastIndexOf(marker)
  if (markerIndex < 0) return null
  const packageStart = markerIndex + marker.length
  const parts = normalized.slice(packageStart).split('/')
  if (!parts[0]) return null
  const name = parts[0].startsWith('@') ? parts.slice(0, 2).join('/') : parts[0]
  return normalized.slice(0, packageStart) + name
}

async function captureProductionBundle() {
  const { build } = await import('vite')
  const moduleIds = new Set()
  const result = await build({
    root: repoRoot,
    logLevel: 'silent',
    build: { write: false },
    plugins: [
      {
        name: 'depoaudio-javascript-notice-capture',
        generateBundle() {
          for (const moduleId of this.getModuleIds()) moduleIds.add(moduleId)
        },
      },
    ],
  })
  const outputs = (Array.isArray(result) ? result : [result]).flatMap(item => item.output ?? [])
  const css = outputs
    .filter(item => item.type === 'asset' && item.fileName.endsWith('.css'))
    .map(item => (typeof item.source === 'string' ? item.source : new TextDecoder().decode(item.source)))
    .join('\n')
  assert(moduleIds.size > 0, 'Vite emitted no module graph')
  assert(css.length > 0, 'Vite emitted no CSS for notice provenance')
  return { moduleIds, css }
}

const validateMatch = (record, expected, label) => {
  assert(record.name === expected.name, `${label} name drifted: ${record.name}`)
  assert(record.version === expected.version, `${label} version drifted: ${record.name}@${record.version}`)
  assert(record.integrity === expected.integrity, `${label} integrity drifted: ${record.name}@${record.version}`)
}

const matchingSupplement = (overrides, record) =>
  overrides.supplements.find(group =>
    group.packages.some(item => item.name === record.name && item.version === record.version),
  )

const matchingException = (overrides, record) =>
  overrides.exceptions.find(item => item.name === record.name && item.version === record.version)

async function loadComponent({ packageRoot, provenance, lockfile, overrides }) {
  const packageJsonPath = path.join(packageRoot, 'package.json')
  const packageJson = await readJson(packageJsonPath)
  const lockPath = portablePath(path.relative(repoRoot, packageRoot))
  assert(lockPath.startsWith('node_modules/'), `Bundled package is outside the npm lock tree: ${packageJson.name}`)
  const lockRecord = lockfile.packages[lockPath]
  assert(lockRecord, `Bundled package is absent from package-lock.json: ${packageJson.name}`)
  assert(packageJson.name && packageJson.version, `Bundled package metadata is incomplete at ${lockPath}`)
  assert(lockRecord.version === packageJson.version, `Installed/locked version mismatch for ${packageJson.name}`)
  assert(typeof lockRecord.integrity === 'string', `Locked integrity is missing for ${packageJson.name}`)
  assert(typeof packageJson.license === 'string', `License expression is missing for ${packageJson.name}`)

  const record = {
    name: packageJson.name,
    version: packageJson.version,
    integrity: lockRecord.integrity,
    license: packageJson.license,
  }
  const legalNames = (await readdir(packageRoot, { withFileTypes: true }))
    .filter(entry => entry.isFile() && legalFilePattern.test(entry.name))
    .map(entry => entry.name)
    .sort((left, right) => left.localeCompare(right, 'en'))
  const materials = []
  for (const filename of legalNames) {
    materials.push(
      await readTextMaterial({
        absolutePath: path.join(packageRoot, filename),
        displayName: filename,
        sourceKind: 'npm-package',
      }),
    )
  }

  const supplement = matchingSupplement(overrides, record)
  if (supplement) {
    const expected = supplement.packages.find(item => item.name === record.name && item.version === record.version)
    validateMatch(record, expected, 'Supplemented package')
    for (const specification of supplement.files) materials.push(await readPinnedMaterial(specification))
  }

  let unresolved = null
  const exception = matchingException(overrides, record)
  if (exception) {
    validateMatch(record, exception, 'Exception package')
    const evidence = exception.currentUpstreamEvidence
    const evidenceBytes = await readFile(resolveInsideRepo(evidence.path))
    assert(sha256(evidenceBytes) === evidence.sha256, `Exception provenance hash mismatch for ${record.name}`)
    assert(/^[0-9a-f]{40}$/u.test(evidence.sourceRevision), `Exception provenance is not immutable`)
    assert(evidence.sourceUrl.includes(evidence.sourceRevision), `Exception provenance URL is not immutable`)
    assert(/^[0-9a-f]{64}$/u.test(evidence.upstreamRawSha256), `Exception provenance has no upstream raw hash`)
    if (exception.status === 'approved') {
      assert(exception.approval && typeof exception.approval.approvedBy === 'string', `Approved exception lacks owner`)
      assert(
        exception.approval && /^\d{4}-\d{2}-\d{2}$/u.test(exception.approval.approvedOn),
        `Approved exception lacks date`,
      )
      const decisionBytes = await readFile(resolveInsideRepo(exception.approval.decisionFile))
      assert(
        sha256(decisionBytes) === exception.approval.decisionSha256,
        `Approved exception decision record does not match for ${record.name}`,
      )
      assert(exception.noticeFiles.length > 0, `Approved exception has no notice files`)
      for (const specification of exception.noticeFiles) materials.push(await readPinnedMaterial(specification))
    } else {
      assert(exception.status === 'unresolved', `Unknown exception status for ${record.name}`)
      unresolved = {
        reason: exception.reason,
        requiredResolution: exception.requiredResolution,
        evidence: {
          sourceUrl: evidence.sourceUrl,
          sourceRevision: evidence.sourceRevision,
          sha256: evidence.sha256,
          applicability: evidence.applicability,
        },
      }
    }
  }

  assert(materials.length > 0 || unresolved, `No complete legal material or reviewed exception for ${record.name}`)
  materials.sort((left, right) =>
    `${left.sourceKind}\0${left.displayName}`.localeCompare(`${right.sourceKind}\0${right.displayName}`, 'en'),
  )
  return {
    ...record,
    provenance: [...provenance].sort(),
    status: unresolved ? 'unresolved' : 'resolved',
    legalMaterials: materials,
    unresolved,
  }
}

const componentKey = component => `${component.name}@${component.version}`

const publicComponent = component => ({
  name: component.name,
  version: component.version,
  integrity: component.integrity,
  license: component.license,
  provenance: component.provenance,
  status: component.status,
  legalMaterials: component.legalMaterials.map(material => ({
    displayName: material.displayName,
    sourceKind: material.sourceKind,
    sourceUrl: material.sourceUrl,
    sourceRevision: material.sourceRevision,
    sourceSha256: material.sourceSha256,
    contentSha256: material.contentSha256,
  })),
  unresolved: component.unresolved,
})

function renderHtml({ components, lockfileSha256 }) {
  const unresolved = components.filter(component => component.status === 'unresolved')
  const rows = components
    .map(
      component =>
        `<tr><td>${escapeHtml(component.name)}</td><td>${escapeHtml(component.version)}</td><td>${escapeHtml(component.license)}</td><td>${escapeHtml(component.provenance.join(', '))}</td><td>${escapeHtml(component.status)}</td></tr>`,
    )
    .join('\n')
  const sections = components
    .map((component, index) => {
      const warning = component.unresolved
        ? `<div class="unresolved"><strong>Unresolved notice.</strong> ${escapeHtml(component.unresolved.reason)}<br><strong>Required resolution:</strong> ${escapeHtml(component.unresolved.requiredResolution)}<br><strong>Current upstream evidence:</strong> <a href="${escapeHtml(component.unresolved.evidence.sourceUrl)}">immutable source</a> at <code>${escapeHtml(component.unresolved.evidence.sourceRevision)}</code>. ${escapeHtml(component.unresolved.evidence.applicability)}</div>`
        : ''
      const materials = component.legalMaterials
        .map(
          material =>
            `<h3>${escapeHtml(material.displayName)}</h3><p class="source">Source: ${escapeHtml(material.sourceKind)}${material.sourceUrl ? ` · <a href="${escapeHtml(material.sourceUrl)}">upstream</a> · <code>${escapeHtml(material.sourceRevision)}</code>` : ''} · SHA-256 <code>${escapeHtml(material.contentSha256)}</code></p><pre>${escapeHtml(material.text)}</pre>`,
        )
        .join('\n')
      return `<section id="component-${index + 1}"><h2>${escapeHtml(component.name)} <small>${escapeHtml(component.version)}</small></h2><p><strong>Declared license:</strong> ${escapeHtml(component.license)} · <strong>Payload:</strong> ${escapeHtml(component.provenance.join(', '))}</p>${warning}${materials}</section>`
    })
    .join('\n')
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>DepoAudio JavaScript and generated-CSS third-party notices</title>
<style>body{font:15px/1.55 system-ui,sans-serif;max-width:78rem;margin:auto;padding:2rem;color:#211b24}table{border-collapse:collapse;width:100%}th,td{border:1px solid #bbb;padding:.4rem;text-align:left;vertical-align:top}pre{white-space:pre-wrap;border:1px solid #ccc;background:#f7f5f2;padding:1rem;overflow-wrap:anywhere}code{overflow-wrap:anywhere}.unresolved{border:2px solid #8a3d21;background:#fff3e9;padding:1rem}.source{font-size:.875rem;color:#514956}section{border-top:3px solid #513554;margin-top:2.5rem;padding-top:1rem}small{font-weight:400}</style>
</head>
<body>
<main>
<h1>DepoAudio JavaScript and generated-CSS third-party notices</h1>
<p>This report is generated deterministically from the locked Vite production graph and reviewed generated-CSS inputs. It contains ${components.length} components. Lockfile SHA-256: <code>${lockfileSha256}</code>.</p>
<p><strong>Unresolved components: ${unresolved.length}.</strong>${unresolved.length ? ' An unresolved record is evidence of a blocked release, not redistribution approval.' : ' All shipped components have reviewed legal material.'}</p>
<table><thead><tr><th>Component</th><th>Version</th><th>Declared license</th><th>Payload provenance</th><th>Status</th></tr></thead><tbody>
${rows}
</tbody></table>
${sections}
</main>
</body>
</html>
`
}

async function generate() {
  const [lockfileBytes, overrides, bundle] = await Promise.all([
    readFile(path.join(repoRoot, 'package-lock.json')),
    readJson(overridePath),
    captureProductionBundle(),
  ])
  const lockfile = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(lockfileBytes))
  assert(lockfile.lockfileVersion === 3, `Expected npm lockfile version 3`)
  assert(overrides.schemaVersion === 1, `Unknown JavaScript notice override schema`)

  const roots = new Map()
  for (const moduleId of bundle.moduleIds) {
    const packageRoot = packageRootFromModule(moduleId)
    if (!packageRoot) continue
    if (!roots.has(packageRoot)) roots.set(packageRoot, new Set())
    roots.get(packageRoot).add('javascript')
  }
  for (const cssPackage of overrides.generatedCss) {
    const input = await readFile(resolveInsideRepo(cssPackage.inputFile), 'utf8')
    for (const evidence of cssPackage.inputContains) {
      assert(input.includes(evidence), `Generated-CSS input evidence is missing for ${cssPackage.name}: ${evidence}`)
    }
    for (const evidence of cssPackage.outputContains) {
      assert(
        bundle.css.includes(evidence),
        `Generated-CSS output evidence is missing for ${cssPackage.name}: ${evidence}`,
      )
    }
    const packageRoot = path.resolve(repoRoot, 'node_modules', ...cssPackage.name.split('/'))
    const packageJson = await readJson(path.join(packageRoot, 'package.json'))
    const lockRecord = lockfile.packages[`node_modules/${cssPackage.name}`]
    assert(lockRecord, `Generated-CSS package is absent from package-lock.json: ${cssPackage.name}`)
    validateMatch(
      { name: packageJson.name, version: packageJson.version, integrity: lockRecord.integrity },
      cssPackage,
      'Generated-CSS package',
    )
    if (!roots.has(packageRoot)) roots.set(packageRoot, new Set())
    roots.get(packageRoot).add('generated-css')
  }

  const components = []
  for (const [packageRoot, provenance] of roots) {
    components.push(await loadComponent({ packageRoot, provenance, lockfile, overrides }))
  }
  components.sort((left, right) => componentKey(left).localeCompare(componentKey(right), 'en'))
  const javascriptCount = components.filter(component => component.provenance.includes('javascript')).length
  const generatedCssCount = components.filter(component => component.provenance.includes('generated-css')).length
  assert(
    javascriptCount === overrides.expected.javascriptComponents,
    `JavaScript closure drifted: expected ${overrides.expected.javascriptComponents}, found ${javascriptCount}`,
  )
  assert(
    generatedCssCount === overrides.expected.generatedCssComponents,
    `Generated-CSS closure drifted: expected ${overrides.expected.generatedCssComponents}, found ${generatedCssCount}`,
  )
  assert(
    components.length === overrides.expected.totalComponents,
    `Total notice closure drifted: expected ${overrides.expected.totalComponents}, found ${components.length}`,
  )

  const componentKeys = new Set(components.map(componentKey))
  for (const group of overrides.supplements) {
    for (const item of group.packages) {
      assert(
        componentKeys.has(`${item.name}@${item.version}`),
        `Stale supplement override for ${item.name}@${item.version}`,
      )
    }
  }
  for (const item of overrides.exceptions) {
    assert(
      componentKeys.has(`${item.name}@${item.version}`),
      `Stale exception override for ${item.name}@${item.version}`,
    )
  }

  const lockfileSha256 = sha256(lockfileBytes)
  const publicComponents = components.map(publicComponent)
  const unresolvedComponents = publicComponents.filter(component => component.status === 'unresolved').map(componentKey)
  const manifest = {
    schemaVersion: 1,
    generator: 'scripts/generate-javascript-notices.mjs',
    lockfileSha256,
    componentCount: publicComponents.length,
    javascriptComponentCount: javascriptCount,
    generatedCssComponentCount: generatedCssCount,
    unresolvedComponentCount: unresolvedComponents.length,
    unresolvedComponents,
    components: publicComponents,
  }
  return {
    html: renderHtml({ components, lockfileSha256 }),
    manifest: `${JSON.stringify(manifest, null, 2)}\n`,
    componentCount: components.length,
    unresolvedComponents,
  }
}

const assertStrict = result => {
  assert(
    result.unresolvedComponents.length === 0,
    `Strict JavaScript notice gate failed: unresolved ${result.unresolvedComponents.join(', ')}`,
  )
}

async function compareCommitted(result) {
  const [committedHtml, committedManifest] = await Promise.all([
    readFile(htmlPath, 'utf8'),
    readFile(manifestPath, 'utf8'),
  ])
  assert(committedHtml === result.html, 'Committed THIRD-PARTY-NOTICES.html is stale')
  assert(committedManifest === result.manifest, 'Committed COMPONENTS.json is stale')
}

async function writeGenerated(result) {
  await mkdir(outputDirectory, { recursive: true })
  await Promise.all([writeFile(htmlPath, result.html, 'utf8'), writeFile(manifestPath, result.manifest, 'utf8')])
}

async function selfTest() {
  assert(legalFilePattern.test('LICENSE'), 'LICENSE was not recognized')
  assert(legalFilePattern.test('licence.md'), 'LICENCE was not recognized')
  assert(legalFilePattern.test('CopyrightNotice.txt'), 'Copyright notice was not recognized')
  assert(!legalFilePattern.test('README.md'), 'README was misclassified as legal material')
  assert(normalizeText('a\r\nb\rc') === 'a\nb\nc', 'Line-ending normalization failed')
  assert(escapeHtml('<a&"\'>') === '&lt;a&amp;&quot;&#39;&gt;', 'HTML escaping failed')
  const unordered = [
    { name: 'z', version: '1.0.0' },
    { name: '@a/b', version: '2.0.0' },
  ]
  const first = [...unordered].sort((left, right) => componentKey(left).localeCompare(componentKey(right), 'en'))
  const second = [...unordered]
    .reverse()
    .sort((left, right) => componentKey(left).localeCompare(componentKey(right), 'en'))
  assert(JSON.stringify(first) === JSON.stringify(second), 'Component ordering is not deterministic')
  let strictRejected = false
  try {
    assertStrict({ unresolvedComponents: ['fixture@1.0.0'] })
  } catch (error) {
    strictRejected = String(error).includes('unresolved fixture@1.0.0')
  }
  assert(strictRejected, 'Strict mode accepted an unresolved notice')
  const overrides = await readJson(overridePath)
  let stalePinRejected = false
  try {
    await readPinnedMaterial({ ...overrides.supplements[0].files[0], sha256: '0'.repeat(64) })
  } catch (error) {
    stalePinRejected = String(error).includes('hash mismatch')
  }
  assert(stalePinRejected, 'Pinned override accepted the wrong file hash')
  assert(
    packageRootFromModule('/repo/node_modules/@scope/package/dist/index.js') === '/repo/node_modules/@scope/package',
    'Scoped package root capture failed',
  )
  for (const group of overrides.supplements) {
    for (const file of group.files) await readPinnedMaterial(file)
  }
  for (const exception of overrides.exceptions) {
    const evidence = exception.currentUpstreamEvidence
    const bytes = await readFile(resolveInsideRepo(evidence.path))
    assert(sha256(bytes) === evidence.sha256, `Exception provenance self-test failed for ${exception.name}`)
    assert(exception.status === 'unresolved', `Self-test fixture unexpectedly approves ${exception.name}`)
  }
  console.log('JavaScript notice self-test passed')
}

async function main() {
  const argumentsSet = new Set(process.argv.slice(2))
  for (const argument of argumentsSet) {
    assert(['--check', '--strict', '--self-test'].includes(argument), `Unknown argument ${argument}`)
  }
  if (argumentsSet.has('--self-test')) {
    await selfTest()
    return
  }
  const result = await generate()
  if (argumentsSet.has('--strict')) assertStrict(result)
  if (argumentsSet.has('--check')) {
    await compareCommitted(result)
    console.log(
      `JavaScript notices are current (${result.componentCount} components; ${result.unresolvedComponents.length} unresolved)`,
    )
  } else {
    await writeGenerated(result)
    console.log(
      `Generated JavaScript notices (${result.componentCount} components; ${result.unresolvedComponents.length} unresolved)`,
    )
  }
}

main().catch(error => {
  console.error(error instanceof Error ? error.message : String(error))
  process.exitCode = 1
})
