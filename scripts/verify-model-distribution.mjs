import { createHash } from 'node:crypto'
import { lstat, mkdir, mkdtemp, readFile, readdir, readlink, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'

const KNOWN_MODEL_FILENAMES = new Set([
  'dccrn_plus.onnx',
  'dfn3_config.ini',
  'dfn3_df_dec.onnx',
  'dfn3_enc.onnx',
  'dfn3_erb_dec.onnx',
  'dnsmos_sig_bak_ovr.onnx',
  'flashsr.onnx',
  'silero_vad.onnx',
  'smart-turn-v3-int8.onnx',
  'speaker_embed.onnx',
  'speaker_seg_int8.onnx',
  'weights.rnn',
])

const KNOWN_MODEL_SHA256 = new Set([
  '1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b',
  '23114ce3b0f6464b763ee62f7bb8aab6b2a129a21eabd5bcfe59413db05f278a',
  '269fbebdb513aa23cddfbb593542ecc540284a91849ac50516870e1ac78f6edd',
  '3d072c8fb04446955a365b533686e7e06015ad09929bb824b910c72ff89f5be1',
  '7c5399d3da8a50ebef1c1a0ae421b33376aa5e45d0e92df16da7e83c9c131916',
  'a4a068cd6cf1ea8355b84327595838ca748ec29a25bc91fc82e6c299ccdc5808',
  'ab669a1d10afe20911728b33053a452071042317a90581092b325da7b2f9d895',
  'd582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d',
  'e255c76b227f16f7f392cc43677c38bd2c5aa129f042a2ba3eb03fb29e470c7a',
  // nnnoiseless 0.5.2 src/weights.rnn, compiled into the application when used.
  'e6de5fbfadf7ec91d1b24d6a6ccfd0290cb4d8bf555c5eab3ce41506f67a58b1',
])

const LEARNED_MODULES = ['denoise', 'dereverb', 'enhance', 'mel', 'scoring', 'speakers', 'vad']
const LEARNED_COMMANDS = [
  'detect_speakers_cmd',
  'detect_speech_cmd',
  'download_model_cmd',
  'model_catalog_cmd',
  'score_quality_cmd',
]

const portable = value => value.split(path.sep).join('/')
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex')

async function readRequired(root, relative) {
  try {
    return await readFile(path.join(root, ...relative.split('/')), 'utf8')
  } catch (error) {
    throw new Error(`Model distribution contract cannot read ${relative}: ${error.message}`)
  }
}

async function collectFiles(root, directory = root) {
  const files = []
  let children
  try {
    children = await readdir(directory, { withFileTypes: true })
  } catch (error) {
    if (error.code === 'ENOENT') return files
    throw error
  }
  children.sort((left, right) => left.name.localeCompare(right.name, 'en'))
  for (const child of children) {
    const absolute = path.join(directory, child.name)
    if (child.isDirectory()) files.push(...(await collectFiles(root, absolute)))
    else if (child.isFile()) files.push({ absolute, relative: portable(path.relative(root, absolute)), type: 'file' })
    else if (child.isSymbolicLink()) {
      files.push({
        absolute,
        relative: portable(path.relative(root, absolute)),
        target: portable(await readlink(absolute)),
        type: 'symlink',
      })
    }
  }
  return files
}

const hasModelPath = relative => {
  const normalized = `/${portable(relative).toLowerCase()}/`
  const basename = path.posix.basename(normalized.slice(0, -1))
  return (
    normalized.endsWith('.onnx/') ||
    /\/(?:model|models|onnxruntime)\//u.test(normalized) ||
    KNOWN_MODEL_FILENAMES.has(basename)
  )
}

const isForbiddenResourceEntry = file =>
  hasModelPath(file.relative) || (file.type === 'symlink' && hasModelPath(file.target))

async function assertNoModelMaterial(root, relativeRoot) {
  const files = await collectFiles(path.join(root, ...relativeRoot.split('/')))
  for (const file of files) {
    if (isForbiddenResourceEntry(file)) {
      throw new Error(`Learned-model material is forbidden in v1.0.3: ${relativeRoot}/${file.relative}`)
    }
    if (file.type === 'symlink') {
      throw new Error(`Symbolic links are forbidden in verified v1.0.3 resources: ${relativeRoot}/${file.relative}`)
    }
    const hash = sha256(await readFile(file.absolute))
    if (KNOWN_MODEL_SHA256.has(hash)) {
      throw new Error(`Known learned-model bytes are forbidden in v1.0.3: ${relativeRoot}/${file.relative}`)
    }
  }
}

function assertCargoBoundary(cargoToml, cargoLock) {
  for (const dependency of ['nnnoiseless', 'ort', 'ort-sys']) {
    const directPattern = new RegExp(`^${dependency}\\s*=`, 'mu')
    const lockPattern = new RegExp(`^name = "${dependency}"$`, 'mu')
    if (directPattern.test(cargoToml) || lockPattern.test(cargoLock)) {
      throw new Error(`Learned-model runtime dependency remains in the release graph: ${dependency}`)
    }
  }
}

function assertCompiledBoundary(libSource, commandsSource) {
  for (const moduleName of LEARNED_MODULES) {
    if (new RegExp(`^mod ${moduleName};`, 'mu').test(libSource)) {
      throw new Error(`Learned-model module remains compiled: ${moduleName}`)
    }
  }
  for (const command of LEARNED_COMMANDS) {
    if (libSource.includes(command) || new RegExp(`pub (?:async )?fn ${command}\\b`, 'u').test(commandsSource)) {
      throw new Error(`Learned-model command remains exposed: ${command}`)
    }
  }
}

function assertConversionBoundary(conversionSource) {
  for (const learnedImport of ['crate::denoise', 'crate::dereverb', 'crate::enhance', 'nnnoiseless']) {
    if (conversionSource.includes(learnedImport)) {
      throw new Error(`Conversion still links learned-model processing: ${learnedImport}`)
    }
  }
  const guard = /if\s+job\.denoise\s*\|\|\s*job\.enhance\s*\|\|\s*job\.dereverb\s*\{/u.exec(conversionSource)
  if (!guard) throw new Error('Conversion does not reject all learned-model request flags')
  const errorIndex = conversionSource.indexOf('Learned-model processing is not included', guard.index)
  if (errorIndex < 0) throw new Error('Conversion learned-model rejection is not explicit')
  const firstInputSideEffect = [
    conversionSource.indexOf('safe_input_path('),
    conversionSource.indexOf('std::fs::metadata('),
    conversionSource.indexOf('Command::new('),
  ]
    .filter(index => index >= 0)
    .sort((left, right) => left - right)[0]
  if (firstInputSideEffect !== undefined && guard.index > firstInputSideEffect) {
    throw new Error('Conversion learned-model flags are rejected only after input side effects')
  }
}

function assertFrontendBoundary(sourceFiles) {
  const combined = sourceFiles.join('\n')
  for (const flag of ['denoise', 'enhance', 'dereverb']) {
    if (new RegExp(`${flag}\\s*:\\s*true\\b`, 'u').test(combined)) {
      throw new Error(`Frontend can still enable learned-model processing: ${flag}`)
    }
  }
  for (const command of ['download_model_cmd', 'model_catalog_cmd']) {
    if (combined.includes(command)) throw new Error(`Frontend can still invoke learned-model command: ${command}`)
  }
}

function assertCapabilitiesBoundary(modelsSource) {
  for (const contract of [
    'recommend_speaker_detection: false',
    'recommend_enhance: false',
    'dereverb_available: false',
    'recommended_denoise: "unavailable".into()',
  ]) {
    if (!modelsSource.includes(contract)) throw new Error(`System capabilities do not fail closed: ${contract}`)
  }
  if (/(?:https?:\/\/|download_model|reqwest::)/u.test(modelsSource)) {
    throw new Error('The v1.0.3 model surface contains a download path')
  }
}

async function verifyRepository(root = process.cwd()) {
  const [cargoToml, cargoLock, libSource, commandsSource, conversionSource, modelsSource, tauriConfig] =
    await Promise.all([
      readRequired(root, 'src-tauri/Cargo.toml'),
      readRequired(root, 'src-tauri/Cargo.lock'),
      readRequired(root, 'src-tauri/src/lib.rs'),
      readRequired(root, 'src-tauri/src/commands.rs'),
      readRequired(root, 'src-tauri/src/conversion.rs'),
      readRequired(root, 'src-tauri/src/models.rs'),
      readRequired(root, 'src-tauri/tauri.conf.json'),
    ])

  assertCargoBoundary(cargoToml, cargoLock)
  assertCompiledBoundary(libSource, commandsSource)
  assertConversionBoundary(conversionSource)
  assertCapabilitiesBoundary(modelsSource)

  const tauri = JSON.parse(tauriConfig)
  const resources = tauri?.bundle?.resources
  if (!Array.isArray(resources)) throw new Error('Tauri bundle resources must be an explicit array')
  const forbiddenResource = resources.find(resource => hasModelPath(String(resource)))
  if (forbiddenResource) throw new Error(`Tauri bundles learned-model material: ${forbiddenResource}`)

  await assertNoModelMaterial(root, 'src-tauri/resources')
  const frontendFiles = await collectFiles(path.join(root, 'src'))
  const frontendSource = await Promise.all(
    frontendFiles
      .filter(
        file =>
          file.type === 'file' &&
          /\.(?:js|jsx|ts|tsx)$/u.test(file.relative) &&
          !file.relative.includes('__tests__/') &&
          !/\.(?:test|spec)\.[^.]+$/u.test(file.relative),
      )
      .map(file => readFile(file.absolute, 'utf8')),
  )
  assertFrontendBoundary(frontendSource)
}

async function writeFixture(root) {
  const files = {
    'src-tauri/Cargo.toml': '[package]\nname = "fixture"\nversion = "1.0.3"\n',
    'src-tauri/Cargo.lock': 'version = 4\n',
    'src-tauri/src/lib.rs': 'mod conversion;\nmod models;\n',
    'src-tauri/src/commands.rs': 'pub fn health_cmd() {}\n',
    'src-tauri/src/conversion.rs':
      'fn validate(job: Job) -> Result<(), String> {\n  if job.denoise || job.enhance || job.dereverb { return Err("Learned-model processing is not included".into()); }\n  safe_input_path(job.input)?;\n  Ok(())\n}\n',
    'src-tauri/src/models.rs':
      'recommended_denoise: "unavailable".into(),\nrecommend_speaker_detection: false,\nrecommend_enhance: false,\ndereverb_available: false,\n',
    'src-tauri/tauri.conf.json': '{"bundle":{"resources":["resources/third-party/javascript"]}}\n',
    'src/presets.js': 'export const preset = { denoise: false, enhance: false, dereverb: false }\n',
    'src-tauri/resources/third-party/javascript/notice.txt': 'fixture notice\n',
  }
  for (const [relative, content] of Object.entries(files)) {
    const destination = path.join(root, ...relative.split('/'))
    await mkdir(path.dirname(destination), { recursive: true })
    await writeFile(destination, content)
  }
}

async function expectFixtureRejection(root, relative, content, expectedMessage) {
  const destination = path.join(root, ...relative.split('/'))
  const original = await readFile(destination)
  await writeFile(destination, content)
  let rejected = false
  try {
    await verifyRepository(root)
  } catch (error) {
    rejected = String(error).includes(expectedMessage)
  }
  await writeFile(destination, original)
  if (!rejected) throw new Error(`Model distribution self-test did not reject ${relative}`)
}

async function selfTest() {
  const root = await mkdtemp(path.join(tmpdir(), 'depoaudio-model-contract-'))
  try {
    await writeFixture(root)
    await verifyRepository(root)
    await expectFixtureRejection(root, 'src-tauri/Cargo.toml', 'nnnoiseless = "0.5.2"\n', 'nnnoiseless')
    await expectFixtureRejection(root, 'src-tauri/src/lib.rs', 'mod denoise;\n', 'module remains compiled')
    await expectFixtureRejection(
      root,
      'src-tauri/src/conversion.rs',
      'fn convert(job: Job) { safe_input_path(job.input); }\n',
      'does not reject all learned-model request flags',
    )
    await expectFixtureRejection(
      root,
      'src/presets.js',
      'export const preset = { denoise: true }\n',
      'Frontend can still enable learned-model processing',
    )
    const modelPath = path.join(root, 'src-tauri', 'resources', 'models', 'renamed.onnx')
    await mkdir(path.dirname(modelPath), { recursive: true })
    await writeFile(modelPath, 'model bytes\n')
    let rejected = false
    try {
      await verifyRepository(root)
    } catch (error) {
      rejected = String(error).includes('Learned-model material is forbidden')
    }
    if (!rejected) throw new Error('Model distribution self-test did not reject packaged ONNX material')
    if (!KNOWN_MODEL_SHA256.has('e6de5fbfadf7ec91d1b24d6a6ccfd0290cb4d8bf555c5eab3ce41506f67a58b1')) {
      throw new Error('Model distribution self-test lost the embedded RNNoise weights hash')
    }
    if (!isForbiddenResourceEntry({ type: 'symlink', relative: 'safe-link', target: 'models/renamed.bin' })) {
      throw new Error('Model distribution self-test did not reject a forbidden symlink target')
    }
    console.log('Model distribution contract self-test OK')
  } finally {
    await rm(root, { recursive: true, force: true })
  }
}

const args = process.argv.slice(2)
if (args.includes('--self-test')) await selfTest()
else {
  const artifactIndex = args.indexOf('--artifact-root')
  if (artifactIndex >= 0) {
    const artifactRoot = args[artifactIndex + 1]
    if (!artifactRoot) throw new Error('Missing required --artifact-root value')
    const metadata = await lstat(path.resolve(artifactRoot))
    if (!metadata.isDirectory()) throw new Error('--artifact-root must name a directory')
    await assertNoModelMaterial(path.dirname(path.resolve(artifactRoot)), path.basename(path.resolve(artifactRoot)))
  } else {
    await verifyRepository()
  }
  console.log('Model distribution contract verified')
}
