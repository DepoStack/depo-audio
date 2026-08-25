import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'

const args = process.argv.slice(2)
const valueAfter = flag => {
  const index = args.indexOf(flag)
  if (index < 0 || !args[index + 1]) throw new Error(`Missing required ${flag} value`)
  return args[index + 1]
}

const packageManifest = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8'))

const replaceRustWorkspaceRefs = (value, oldRoot, stableRoot) => {
  if (typeof value === 'string')
    return value.startsWith(oldRoot) ? `${stableRoot}${value.slice(oldRoot.length)}` : value
  if (Array.isArray(value)) return value.map(item => replaceRustWorkspaceRefs(item, oldRoot, stableRoot))
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, replaceRustWorkspaceRefs(item, oldRoot, stableRoot)]),
    )
  }
  return value
}

const normalizeDocument = (document, { sourceCommit, epochText, ecosystem }) => {
  if (!/^[0-9a-f]{40}$/u.test(sourceCommit)) {
    throw new Error('--source-commit must be a full lowercase Git commit SHA')
  }
  if (!/^[1-9][0-9]*$/u.test(epochText)) {
    throw new Error('--source-date-epoch must be a positive integer')
  }
  if (!['npm', 'rust'].includes(ecosystem)) {
    throw new Error('--ecosystem must be npm or rust')
  }

  const sourceDate = new Date(Number(epochText) * 1_000)
  if (Number.isNaN(sourceDate.valueOf())) throw new Error('--source-date-epoch is outside the supported date range')

  if (document.bomFormat !== 'CycloneDX') throw new Error('SBOM is not a CycloneDX document')
  if (document.specVersion !== '1.5') throw new Error('Release SBOM must use CycloneDX specification version 1.5')
  if (!Array.isArray(document.components) || document.components.length === 0) {
    throw new Error('SBOM contains no dependency components')
  }
  for (const component of document.components) {
    if (!component?.name || !component?.version) throw new Error('SBOM contains a component without name and version')
  }

  if (ecosystem === 'rust') {
    const root = document.metadata?.component
    if (root?.name !== packageManifest.name || root?.version !== packageManifest.version) {
      throw new Error('Rust SBOM root component does not match the current DepoAudio package')
    }
    const oldRoot = root['bom-ref']
    const expectedSuffix = `#${packageManifest.name}@${packageManifest.version}`
    if (typeof oldRoot !== 'string' || !oldRoot.startsWith('path+file://') || !oldRoot.endsWith(expectedSuffix)) {
      throw new Error('Rust SBOM root does not contain the expected cargo-cyclonedx workspace reference')
    }
    const stableRoot = `urn:depoaudio:cargo:${sourceCommit}:${packageManifest.name}@${packageManifest.version}`
    document = replaceRustWorkspaceRefs(document, oldRoot, stableRoot)
    if (JSON.stringify(document).includes('path+file://')) {
      throw new Error('Rust SBOM still contains a checkout-specific path reference')
    }
  } else {
    const root = document.metadata?.component
    const expectedPurl = `pkg:npm/${packageManifest.name}@${packageManifest.version}`
    const expectedRef = `${packageManifest.name}@${packageManifest.version}`
    if (root?.version !== packageManifest.version || root?.purl !== expectedPurl || root?.['bom-ref'] !== expectedRef) {
      throw new Error('npm SBOM root component does not match the current DepoAudio package')
    }
    root.name = packageManifest.name
    if (JSON.stringify(document).includes('path+file://')) {
      throw new Error('npm SBOM contains a checkout-specific path reference')
    }
  }

  // Both generators can otherwise introduce wall-clock time or a random UUID.
  // Tie the evidence document to the immutable release commit instead.
  delete document.serialNumber
  document.metadata ??= {}
  document.metadata.timestamp = sourceDate.toISOString()
  const properties = Array.isArray(document.metadata.properties)
    ? document.metadata.properties.filter(property => !String(property?.name).startsWith('depoaudio:'))
    : []
  properties.push(
    { name: 'depoaudio:ecosystem', value: ecosystem },
    { name: 'depoaudio:source-commit', value: sourceCommit },
  )
  properties.sort((left, right) => left.name.localeCompare(right.name, 'en'))
  document.metadata.properties = properties
  return document
}

const runSelfTest = () => {
  const sourceCommit = '0123456789abcdef0123456789abcdef01234567'
  const epochText = '1700000000'
  const fixture = root => ({
    bomFormat: 'CycloneDX',
    specVersion: '1.5',
    serialNumber: 'urn:uuid:random',
    metadata: {
      timestamp: '2026-01-01T00:00:00.000Z',
      component: {
        type: 'application',
        name: packageManifest.name,
        version: packageManifest.version,
        'bom-ref': root,
        components: [
          {
            type: 'application',
            name: 'depo-audio',
            version: packageManifest.version,
            'bom-ref': `${root} bin-target-0`,
          },
        ],
      },
    },
    components: [{ type: 'library', name: 'serde', version: '1.0.0', 'bom-ref': 'pkg:cargo/serde@1.0.0' }],
    dependencies: [{ ref: root, dependsOn: ['pkg:cargo/serde@1.0.0'] }],
  })
  const leftRoot = `path+file:///tmp/checkout-a/src-tauri#${packageManifest.name}@${packageManifest.version}`
  const rightRoot = `path+file:///opt/checkout-b/src-tauri#${packageManifest.name}@${packageManifest.version}`
  const options = { sourceCommit, epochText, ecosystem: 'rust' }
  const left = normalizeDocument(fixture(leftRoot), options)
  const right = normalizeDocument(fixture(rightRoot), options)
  assert.deepEqual(left, right)
  assert.equal(JSON.stringify(left).includes('path+file://'), false)

  const npmFixture = name => ({
    bomFormat: 'CycloneDX',
    specVersion: '1.5',
    metadata: {
      component: {
        type: 'application',
        name,
        version: packageManifest.version,
        'bom-ref': `${packageManifest.name}@${packageManifest.version}`,
        purl: `pkg:npm/${packageManifest.name}@${packageManifest.version}`,
      },
    },
    components: [{ type: 'library', name: 'react', version: '19.0.0' }],
  })
  const npmLeft = normalizeDocument(npmFixture('checkout-a'), { sourceCommit, epochText, ecosystem: 'npm' })
  const npmRight = normalizeDocument(npmFixture('checkout-b'), { sourceCommit, epochText, ecosystem: 'npm' })
  assert.deepEqual(npmLeft, npmRight)
  assert.equal(npmLeft.metadata.component.name, packageManifest.name)
  console.log('Release SBOM normalizer self-test passed')
}

if (args.includes('--self-test')) {
  runSelfTest()
  process.exit(0)
}

const input = valueAfter('--input')
const output = valueAfter('--output')
const sourceCommit = valueAfter('--source-commit')
const epochText = valueAfter('--source-date-epoch')
const ecosystem = valueAfter('--ecosystem')
const document = normalizeDocument(JSON.parse(await readFile(input, 'utf8')), {
  sourceCommit,
  epochText,
  ecosystem,
})

await writeFile(output, `${JSON.stringify(document, null, 2)}\n`, 'utf8')
console.log(`Normalized ${ecosystem} CycloneDX ${document.specVersion} SBOM: ${document.components.length} components`)
