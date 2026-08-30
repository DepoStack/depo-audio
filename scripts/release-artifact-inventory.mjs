import { createHash } from 'node:crypto'
import { createReadStream } from 'node:fs'
import { lstat, mkdir, mkdtemp, readFile, readlink, readdir, rename, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'

const FORBIDDEN_MODEL_FILENAMES = new Set([
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

const FORBIDDEN_MODEL_SHA256 = new Set([
  '1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b',
  '23114ce3b0f6464b763ee62f7bb8aab6b2a129a21eabd5bcfe59413db05f278a',
  '269fbebdb513aa23cddfbb593542ecc540284a91849ac50516870e1ac78f6edd',
  '3d072c8fb04446955a365b533686e7e06015ad09929bb824b910c72ff89f5be1',
  '7c5399d3da8a50ebef1c1a0ae421b33376aa5e45d0e92df16da7e83c9c131916',
  'a4a068cd6cf1ea8355b84327595838ca748ec29a25bc91fc82e6c299ccdc5808',
  'ab669a1d10afe20911728b33053a452071042317a90581092b325da7b2f9d895',
  'd582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d',
  'e255c76b227f16f7f392cc43677c38bd2c5aa129f042a2ba3eb03fb29e470c7a',
  // nnnoiseless 0.5.2 src/weights.rnn.
  'e6de5fbfadf7ec91d1b24d6a6ccfd0290cb4d8bf555c5eab3ce41506f67a58b1',
])

const sha256File = filePath =>
  new Promise((resolve, reject) => {
    const hash = createHash('sha256')
    const input = createReadStream(filePath)
    input.on('error', reject)
    input.on('data', chunk => hash.update(chunk))
    input.on('end', () => resolve(hash.digest('hex')))
  })

const portablePath = value => value.split(path.sep).join('/')

const hasForbiddenModelPath = value => {
  const normalized = `/${portablePath(value).toLowerCase()}/`
  const basename = path.posix.basename(normalized.slice(0, -1))
  return (
    normalized.endsWith('.onnx/') ||
    /\/(?:model|models|onnxruntime)\//u.test(normalized) ||
    FORBIDDEN_MODEL_FILENAMES.has(basename)
  )
}

const isForbiddenModelEntry = entry =>
  hasForbiddenModelPath(entry.path) ||
  (entry.type === 'symlink' && hasForbiddenModelPath(entry.target)) ||
  FORBIDDEN_MODEL_SHA256.has(entry.sha256)

const isForbiddenFtrEntry = entry =>
  entry.path.toLowerCase().endsWith('.trm') ||
  (entry.type === 'symlink' && entry.target.toLowerCase().endsWith('.trm'))

const stagedBinarySpecs = platform => {
  const noticeRoot =
    platform === 'macos'
      ? '@app/Contents/Resources/resources/third-party/javascript'
      : '@app/resources/third-party/javascript'
  return [
    ...(platform === 'macos'
      ? [
          [
            'ffmpeg',
            'binaries/ffmpeg-universal-apple-darwin',
            'ffmpeg',
            '@app/Contents/MacOS/ffmpeg',
            'packaged-smoke',
          ],
          [
            'ffprobe',
            'binaries/ffprobe-universal-apple-darwin',
            'ffprobe',
            '@app/Contents/MacOS/ffprobe',
            'packaged-smoke',
          ],
        ]
      : [
          ['ffmpeg', 'binaries/ffmpeg-x86_64-pc-windows-msvc.exe', 'ffmpeg.exe', '@app/ffmpeg.exe', 'packaged-smoke'],
          [
            'ffprobe',
            'binaries/ffprobe-x86_64-pc-windows-msvc.exe',
            'ffprobe.exe',
            '@app/ffprobe.exe',
            'packaged-smoke',
          ],
        ]),
    [
      'javascript-notices-html',
      'resources/third-party/javascript/THIRD-PARTY-NOTICES.html',
      'THIRD-PARTY-NOTICES.html',
      `${noticeRoot}/THIRD-PARTY-NOTICES.html`,
      'byte-exact',
    ],
    [
      'javascript-notices-components',
      'resources/third-party/javascript/COMPONENTS.json',
      'COMPONENTS.json',
      `${noticeRoot}/COMPONENTS.json`,
      'byte-exact',
    ],
  ]
}

async function createBinaryContract({ root, output, platform, sourceCommit, releaseTag }) {
  if (!['macos', 'windows'].includes(platform)) throw new Error(`Unsupported platform ${JSON.stringify(platform)}`)
  if (!/^[0-9a-f]{40}$/u.test(sourceCommit)) throw new Error('Binary contract requires a full source commit SHA')
  if (!releaseTag) throw new Error('Binary contract requires a release tag')

  const rootPath = path.resolve(root)
  const files = []
  for (const [role, stagedPath, packagedBasename, packagedPath, verification] of stagedBinarySpecs(platform)) {
    const absolute = path.join(rootPath, ...stagedPath.split('/'))
    const metadata = await lstat(absolute)
    if (!metadata.isFile()) throw new Error(`Binary contract input is not a file: ${stagedPath}`)
    files.push({
      role,
      stagedPath,
      packagedBasename,
      packagedPath,
      verification,
      bytes: metadata.size,
      sha256: await sha256File(absolute),
    })
  }

  const contract = { schemaVersion: 1, platform, sourceCommit, releaseTag, files }
  await mkdir(path.dirname(path.resolve(output)), { recursive: true })
  await writeFile(output, `${JSON.stringify(contract, null, 2)}\n`, 'utf8')
  return contract
}

async function collectEntries(root, directory = root) {
  const entries = []
  const children = await readdir(directory, { withFileTypes: true })
  children.sort((left, right) => left.name.localeCompare(right.name, 'en'))
  for (const child of children) {
    const absolute = path.join(directory, child.name)
    const relative = portablePath(path.relative(root, absolute))
    const metadata = await lstat(absolute)
    if (metadata.isSymbolicLink()) {
      entries.push({ path: relative, type: 'symlink', target: portablePath(await readlink(absolute)) })
    } else if (metadata.isDirectory()) {
      entries.push(...(await collectEntries(root, absolute)))
    } else if (metadata.isFile()) {
      entries.push({ path: relative, type: 'file', bytes: metadata.size, sha256: await sha256File(absolute) })
    }
  }
  return entries
}

const resolveApplicationRoot = (files, platform) => {
  if (platform === 'macos') {
    if (!files.some(entry => entry.path === 'Contents/MacOS/depo-audio')) {
      throw new Error('Packaged macOS app executable is missing from Contents/MacOS')
    }
    return ''
  }
  const executables = files.filter(entry => path.posix.basename(entry.path).toLowerCase() === 'depo-audio.exe')
  if (executables.length !== 1) {
    throw new Error(`Expected exactly one packaged DepoAudio executable; found ${executables.length}`)
  }
  const directory = path.posix.dirname(executables[0].path)
  return directory === '.' ? '' : directory
}

const resolvePackagedPath = (value, appRoot) => {
  if (!value.startsWith('@app/')) throw new Error('Release binary contract packaged path is malformed')
  const suffix = value.slice('@app/'.length)
  return appRoot ? path.posix.join(appRoot, suffix) : suffix
}

function validateBinaryContract(binaryContract, platform) {
  if (
    binaryContract.schemaVersion !== 1 ||
    binaryContract.platform !== platform ||
    !/^[0-9a-f]{40}$/u.test(binaryContract.sourceCommit) ||
    !binaryContract.releaseTag ||
    !Array.isArray(binaryContract.files)
  ) {
    throw new Error('Release binary contract is malformed or for the wrong platform')
  }
  if (process.env.GITHUB_SHA && binaryContract.sourceCommit !== process.env.GITHUB_SHA) {
    throw new Error('Release binary contract source commit does not match GITHUB_SHA')
  }
  if (process.env.RELEASE_TAG && binaryContract.releaseTag !== process.env.RELEASE_TAG) {
    throw new Error('Release binary contract tag does not match RELEASE_TAG')
  }
}

function validateContractFiles(binaryContract, platform) {
  const specs = stagedBinarySpecs(platform)
  if (binaryContract.files.length !== specs.length) {
    throw new Error(`Release binary contract must contain exactly ${specs.length} canonical files`)
  }
  const expectedByRole = new Map(
    specs.map(([role, stagedPath, packagedBasename, packagedPath, verification]) => [
      role,
      { stagedPath, packagedBasename, packagedPath, verification },
    ]),
  )
  const roles = new Set()
  for (const entry of binaryContract.files) {
    const canonical = expectedByRole.get(entry?.role)
    if (
      !canonical ||
      roles.has(entry.role) ||
      entry.stagedPath !== canonical.stagedPath ||
      entry.packagedBasename !== canonical.packagedBasename ||
      entry.packagedPath !== canonical.packagedPath ||
      entry.verification !== canonical.verification ||
      !Number.isSafeInteger(entry.bytes) ||
      entry.bytes <= 0 ||
      !/^[0-9a-f]{64}$/u.test(entry.sha256)
    ) {
      throw new Error('Release binary contract contains a malformed or noncanonical file entry')
    }
    roles.add(entry.role)
  }
}

function requireDistributionNotices(files, platform) {
  const paths = files.map(entry => entry.path.toLowerCase())
  const hasRustReport = paths.some(
    item => item.includes('third-party/rust/') && path.posix.basename(item) === 'third-party-licenses.html',
  )
  if (!hasRustReport) throw new Error('Missing generated Rust third-party license report')

  const ffmpegRoot = 'third-party/ffmpeg/'
  const hasSource = paths.some(item => item.includes(ffmpegRoot) && path.posix.basename(item) === 'source.txt')
  if (platform === 'windows') {
    const hasLicense = paths.some(item => item.includes(ffmpegRoot) && path.posix.basename(item) === 'license.txt')
    const hasConfiguration = paths.some(
      item => item.includes(ffmpegRoot) && path.posix.basename(item) === 'build-configuration.txt',
    )
    if (!hasLicense || !hasConfiguration || !hasSource) {
      throw new Error(
        `Missing FFmpeg distribution material (license=${hasLicense}, configuration=${hasConfiguration}, source=${hasSource})`,
      )
    }
    return
  }

  const required = [
    'ffmpeg-license.md',
    'ffmpeg-copying.lgplv2.1',
    'ffmpeg-copying.lgplv3',
    'lame-copying',
    'lame-license',
    'opus-copying',
    'build-configuration-arm64.txt',
    'build-configuration-x86_64.txt',
    'build-toolchain.txt',
  ]
  const missing = required.filter(
    expected => !paths.some(item => item.includes(ffmpegRoot) && path.posix.basename(item) === expected),
  )
  if (!hasSource || missing.length > 0) {
    throw new Error(`Missing macOS FFmpeg distribution material (source=${hasSource}, missing=${missing.join(',')})`)
  }
}

async function createInventory({ root, output, platform, contract }) {
  if (!['macos', 'windows'].includes(platform)) throw new Error(`Unsupported platform ${JSON.stringify(platform)}`)
  const rootPath = path.resolve(root)
  const rootMetadata = await lstat(rootPath)
  if (!rootMetadata.isDirectory()) throw new Error(`Inventory root is not a directory: ${rootPath}`)

  const entries = await collectEntries(rootPath)
  const files = entries.filter(entry => entry.type === 'file')
  if (files.length === 0) throw new Error('Release artifact inventory is empty')
  const ftrFixture = entries.find(isForbiddenFtrEntry)
  if (ftrFixture) throw new Error(`Forbidden FTR fixture material found at ${ftrFixture.path}`)
  const modelMaterial = entries.find(isForbiddenModelEntry)
  if (modelMaterial) throw new Error(`Forbidden learned-model material found at ${modelMaterial.path}`)

  const binaryContract = JSON.parse(await readFile(contract, 'utf8'))
  validateBinaryContract(binaryContract, platform)
  validateContractFiles(binaryContract, platform)
  const appRoot = resolveApplicationRoot(files, platform)

  const contractHash = await sha256File(path.resolve(contract))
  const expectedContractPath =
    platform === 'macos'
      ? 'Contents/Resources/resources/third-party/release/BINARY-CONTRACT.json'
      : path.posix.join(appRoot, 'resources/third-party/release/BINARY-CONTRACT.json')
  const packagedContracts = files.filter(
    entry => path.posix.basename(entry.path).toLowerCase() === 'binary-contract.json',
  )
  if (
    packagedContracts.length !== 1 ||
    packagedContracts[0].path !== expectedContractPath ||
    packagedContracts[0].sha256 !== contractHash
  ) {
    throw new Error('Packaged binary contract is missing, duplicated, or differs from the pre-package contract')
  }

  for (const expected of binaryContract.files) {
    const requiredPath = resolvePackagedPath(expected.packagedPath, appRoot)
    const matches = files.filter(entry => entry.path === requiredPath)
    if (matches.length !== 1) {
      throw new Error(`Expected exactly one packaged ${expected.role} at ${requiredPath}; found ${matches.length}`)
    }
    if (
      expected.verification === 'byte-exact' &&
      (matches[0].bytes !== expected.bytes || matches[0].sha256 !== expected.sha256)
    ) {
      throw new Error(`Packaged ${expected.role} does not match its pre-package byte contract`)
    }
  }

  requireDistributionNotices(files, platform)
  const inventory = {
    schemaVersion: 1,
    platform,
    sourceCommit: process.env.GITHUB_SHA || null,
    releaseTag: process.env.RELEASE_TAG || null,
    rootName: path.basename(rootPath),
    fileCount: files.length,
    symlinkCount: entries.length - files.length,
    totalBytes: files.reduce((total, entry) => total + entry.bytes, 0),
    entries,
  }
  await writeFile(output, `${JSON.stringify(inventory, null, 2)}\n`, 'utf8')
  return inventory
}

async function stageContractFiles(stagingRoot, artifactRoot, platform) {
  for (const [, stagedPath, , packagedPath] of stagedBinarySpecs(platform)) {
    const content = `${stagedPath}\n`
    const staged = path.join(stagingRoot, ...stagedPath.split('/'))
    const packaged = path.join(artifactRoot, ...packagedPath.slice('@app/'.length).split('/'))
    await mkdir(path.dirname(staged), { recursive: true })
    await mkdir(path.dirname(packaged), { recursive: true })
    await writeFile(staged, content)
    await writeFile(packaged, content)
  }
}

async function writeNotices(root, platform) {
  const resourceRoot =
    platform === 'macos' ? path.join(root, 'Contents', 'Resources', 'resources') : path.join(root, 'resources')
  const rustRoot = path.join(resourceRoot, 'third-party', 'rust')
  await mkdir(rustRoot, { recursive: true })
  await writeFile(path.join(rustRoot, 'THIRD-PARTY-LICENSES.html'), 'rust licenses\n')
  const ffmpegRoot = path.join(resourceRoot, 'third-party', 'ffmpeg')
  await mkdir(ffmpegRoot, { recursive: true })
  await writeFile(path.join(ffmpegRoot, 'SOURCE.txt'), 'source\n')
  const names =
    platform === 'windows'
      ? ['LICENSE.txt', 'BUILD-CONFIGURATION.txt']
      : [
          'FFmpeg-LICENSE.md',
          'FFmpeg-COPYING.LGPLv2.1',
          'FFmpeg-COPYING.LGPLv3',
          'LAME-COPYING',
          'LAME-LICENSE',
          'Opus-COPYING',
          'BUILD-CONFIGURATION-arm64.txt',
          'BUILD-CONFIGURATION-x86_64.txt',
          'BUILD-TOOLCHAIN.txt',
        ]
  for (const name of names) await writeFile(path.join(ffmpegRoot, name), 'evidence\n')
}

async function createFixture(temporaryRoot, platform) {
  const stagingRoot = path.join(temporaryRoot, `${platform}-staging`)
  const artifactRoot = path.join(temporaryRoot, `${platform}-artifact`)
  await mkdir(stagingRoot, { recursive: true })
  if (platform === 'macos') {
    const executable = path.join(artifactRoot, 'Contents', 'MacOS', 'depo-audio')
    await mkdir(path.dirname(executable), { recursive: true })
    await writeFile(executable, 'application\n')
  } else {
    await mkdir(artifactRoot, { recursive: true })
    await writeFile(path.join(artifactRoot, 'depo-audio.exe'), 'application\n')
  }
  await stageContractFiles(stagingRoot, artifactRoot, platform)
  await writeNotices(artifactRoot, platform)
  const contractPath = path.join(temporaryRoot, `BINARY-CONTRACT-${platform}.json`)
  const sourceCommit = process.env.GITHUB_SHA ?? '0123456789abcdef0123456789abcdef01234567'
  const releaseTag = process.env.RELEASE_TAG ?? 'v1.0.3-rc.test'
  await createBinaryContract({ root: stagingRoot, output: contractPath, platform, sourceCommit, releaseTag })
  const packagedContract =
    platform === 'macos'
      ? path.join(artifactRoot, 'Contents', 'Resources', 'resources', 'third-party', 'release', 'BINARY-CONTRACT.json')
      : path.join(artifactRoot, 'resources', 'third-party', 'release', 'BINARY-CONTRACT.json')
  await mkdir(path.dirname(packagedContract), { recursive: true })
  await writeFile(packagedContract, await readFile(contractPath))
  return { artifactRoot, contractPath }
}

async function expectInventoryRejection(options, expectedMessage) {
  let rejected = false
  try {
    await createInventory(options)
  } catch (error) {
    rejected = String(error).includes(expectedMessage)
  }
  if (!rejected) throw new Error(`Release inventory self-test did not reject: ${expectedMessage}`)
}

async function selfTest() {
  const temporaryRoot = await mkdtemp(path.join(tmpdir(), 'depoaudio-inventory-test-'))
  try {
    const output = path.join(temporaryRoot, 'inventory.json')
    const windows = await createFixture(temporaryRoot, 'windows')
    await createInventory({
      ...windows,
      root: windows.artifactRoot,
      output,
      platform: 'windows',
      contract: windows.contractPath,
    })

    const notice = path.join(windows.artifactRoot, 'resources', 'third-party', 'javascript', 'COMPONENTS.json')
    const originalNotice = await readFile(notice)
    await writeFile(notice, 'tampered notice\n')
    await expectInventoryRejection(
      { root: windows.artifactRoot, output, platform: 'windows', contract: windows.contractPath },
      'javascript-notices-components does not match',
    )
    await writeFile(notice, originalNotice)

    const misplacedNotice = path.join(windows.artifactRoot, 'COMPONENTS.json')
    await rename(notice, misplacedNotice)
    await expectInventoryRejection(
      { root: windows.artifactRoot, output, platform: 'windows', contract: windows.contractPath },
      'Expected exactly one packaged javascript-notices-components',
    )
    await rename(misplacedNotice, notice)

    const modelRoot = path.join(windows.artifactRoot, 'resources', 'models')
    await mkdir(modelRoot, { recursive: true })
    await writeFile(path.join(modelRoot, 'renamed.bin'), 'model\n')
    await expectInventoryRejection(
      { root: windows.artifactRoot, output, platform: 'windows', contract: windows.contractPath },
      'Forbidden learned-model material',
    )
    await rm(modelRoot, { recursive: true, force: true })

    await writeFile(path.join(windows.artifactRoot, 'unknown.onnx'), 'model\n')
    await expectInventoryRejection(
      { root: windows.artifactRoot, output, platform: 'windows', contract: windows.contractPath },
      'Forbidden learned-model material',
    )
    await rm(path.join(windows.artifactRoot, 'unknown.onnx'))

    await writeFile(path.join(windows.artifactRoot, 'forbidden.trm'), 'private fixture\n')
    await expectInventoryRejection(
      { root: windows.artifactRoot, output, platform: 'windows', contract: windows.contractPath },
      'Forbidden FTR fixture material',
    )
    await rm(path.join(windows.artifactRoot, 'forbidden.trm'))

    if (
      !isForbiddenModelEntry({
        path: 'renamed.bin',
        sha256: 'e6de5fbfadf7ec91d1b24d6a6ccfd0290cb4d8bf555c5eab3ce41506f67a58b1',
      })
    ) {
      throw new Error('Release inventory self-test did not reject known model bytes under a renamed path')
    }
    if (
      !isForbiddenModelEntry({ type: 'symlink', path: 'safe-link', target: 'resources/models/renamed.bin' }) ||
      !isForbiddenFtrEntry({ type: 'symlink', path: 'safe-link', target: '../private-fixture.trm' })
    ) {
      throw new Error('Release inventory self-test did not reject forbidden symlink targets')
    }

    const macos = await createFixture(temporaryRoot, 'macos')
    await createInventory({
      ...macos,
      root: macos.artifactRoot,
      output,
      platform: 'macos',
      contract: macos.contractPath,
    })
    console.log('Release artifact inventory self-test OK')
  } finally {
    await rm(temporaryRoot, { recursive: true, force: true })
  }
}

const args = process.argv.slice(2)
const valueAfter = flag => {
  const index = args.indexOf(flag)
  if (index < 0 || !args[index + 1]) throw new Error(`Missing required ${flag} value`)
  return args[index + 1]
}

if (args.includes('--self-test')) {
  await selfTest()
} else if (args.includes('--write-contract')) {
  const contract = await createBinaryContract({
    root: valueAfter('--root'),
    output: valueAfter('--output'),
    platform: valueAfter('--platform'),
    sourceCommit: process.env.GITHUB_SHA,
    releaseTag: process.env.RELEASE_TAG,
  })
  console.log(`Release binary contract OK: ${contract.files.length} files (${contract.platform})`)
} else {
  const inventory = await createInventory({
    root: valueAfter('--root'),
    output: valueAfter('--output'),
    platform: valueAfter('--platform'),
    contract: valueAfter('--contract'),
  })
  console.log(
    `Release inventory OK: ${inventory.fileCount} files, ${inventory.totalBytes} bytes (${inventory.platform})`,
  )
}
