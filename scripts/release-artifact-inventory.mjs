import { createHash } from 'node:crypto'
import { createReadStream } from 'node:fs'
import { lstat, mkdir, mkdtemp, readFile, readlink, readdir, rename, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'

const RETIRED_MODEL_FILENAMES = new Set([
  'dfn3_config.ini',
  'dfn3_enc.onnx',
  'dfn3_erb_dec.onnx',
  'dfn3_df_dec.onnx',
  'speaker_embed.onnx',
])
const RETIRED_MODEL_SHA256 = new Set([
  '7c5399d3da8a50ebef1c1a0ae421b33376aa5e45d0e92df16da7e83c9c131916',
  'ab669a1d10afe20911728b33053a452071042317a90581092b325da7b2f9d895',
  '23114ce3b0f6464b763ee62f7bb8aab6b2a129a21eabd5bcfe59413db05f278a',
  '1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b',
])
const ACTIVE_MODEL_SHA256 = new Map([
  ['flashsr.onnx', 'e255c76b227f16f7f392cc43677c38bd2c5aa129f042a2ba3eb03fb29e470c7a'],
  ['silero_vad.onnx', 'a4a068cd6cf1ea8355b84327595838ca748ec29a25bc91fc82e6c299ccdc5808'],
  ['smart-turn-v3-int8.onnx', '3d072c8fb04446955a365b533686e7e06015ad09929bb824b910c72ff89f5be1'],
  ['speaker_seg_int8.onnx', 'd582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d'],
])
const isRetiredModelEntry = entry =>
  RETIRED_MODEL_FILENAMES.has(path.posix.basename(entry.path).toLowerCase()) || RETIRED_MODEL_SHA256.has(entry.sha256)

const sha256File = filePath =>
  new Promise((resolve, reject) => {
    const hash = createHash('sha256')
    const input = createReadStream(filePath)
    input.on('error', reject)
    input.on('data', chunk => hash.update(chunk))
    input.on('end', () => resolve(hash.digest('hex')))
  })

const portablePath = value => value.split(path.sep).join('/')

const stagedBinarySpecs = platform => {
  const platformSpecs =
    platform === 'macos'
      ? [
          ['ffmpeg', 'binaries/ffmpeg-universal-apple-darwin', 'ffmpeg', '@app/Contents/MacOS/ffmpeg'],
          ['ffprobe', 'binaries/ffprobe-universal-apple-darwin', 'ffprobe', '@app/Contents/MacOS/ffprobe'],
          [
            'onnxruntime',
            'resources/onnxruntime/libonnxruntime.dylib',
            'libonnxruntime.dylib',
            '@app/Contents/Frameworks/libonnxruntime.dylib',
          ],
        ]
      : [
          ['ffmpeg', 'binaries/ffmpeg-x86_64-pc-windows-msvc.exe', 'ffmpeg.exe', '@app/ffmpeg.exe'],
          ['ffprobe', 'binaries/ffprobe-x86_64-pc-windows-msvc.exe', 'ffprobe.exe', '@app/ffprobe.exe'],
          [
            'onnxruntime',
            'resources/onnxruntime/onnxruntime.dll',
            'onnxruntime.dll',
            '@app/resources/onnxruntime/onnxruntime.dll',
          ],
        ]
  const modelRoot = platform === 'macos' ? '@app/Contents/Resources/resources/models' : '@app/resources/models'
  return [
    ...platformSpecs,
    ...[...ACTIVE_MODEL_SHA256].map(([filename]) => [
      `model:${filename}`,
      `resources/models/${filename}`,
      filename,
      `${modelRoot}/${filename}`,
    ]),
  ]
}

async function createBinaryContract({ root, output, platform, sourceCommit, releaseTag }) {
  if (!['macos', 'windows'].includes(platform)) throw new Error(`Unsupported platform ${JSON.stringify(platform)}`)
  if (!/^[0-9a-f]{40}$/u.test(sourceCommit)) throw new Error('Binary contract requires a full source commit SHA')
  if (!releaseTag) throw new Error('Binary contract requires a release tag')

  const rootPath = path.resolve(root)
  const files = []
  for (const [role, stagedPath, packagedBasename, packagedPath] of stagedBinarySpecs(platform)) {
    const absolute = path.join(rootPath, ...stagedPath.split('/'))
    const metadata = await lstat(absolute)
    if (!metadata.isFile()) throw new Error(`Binary contract input is not a file: ${stagedPath}`)
    const sha256 = await sha256File(absolute)
    const expectedModelHash = ACTIVE_MODEL_SHA256.get(packagedBasename)
    if (expectedModelHash && sha256 !== expectedModelHash) {
      throw new Error(`Active model hash mismatch for ${packagedBasename}: ${sha256}`)
    }
    files.push({
      role,
      stagedPath,
      packagedBasename,
      packagedPath,
      verification: expectedModelHash ? 'byte-exact' : 'packaged-smoke',
      bytes: metadata.size,
      sha256,
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
      entries.push({
        path: relative,
        type: 'file',
        bytes: metadata.size,
        sha256: await sha256File(absolute),
      })
    }
  }
  return entries
}

async function createInventory({ root, output, platform, contract }) {
  if (!['macos', 'windows'].includes(platform)) {
    throw new Error(`Unsupported platform ${JSON.stringify(platform)}`)
  }
  const rootPath = path.resolve(root)
  const rootMetadata = await lstat(rootPath)
  if (!rootMetadata.isDirectory()) throw new Error(`Inventory root is not a directory: ${rootPath}`)

  const entries = await collectEntries(rootPath)
  const files = entries.filter(entry => entry.type === 'file')
  if (files.length === 0) throw new Error('Release artifact inventory is empty')

  const forbidden = files.find(entry => entry.path.toLowerCase().endsWith('.trm'))
  if (forbidden) throw new Error(`Forbidden FTR fixture material found at ${forbidden.path}`)

  const retiredModel = files.find(isRetiredModelEntry)
  if (retiredModel) throw new Error(`Retired model material found at ${retiredModel.path}`)

  const binaryContract = JSON.parse(await readFile(contract, 'utf8'))
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

  const contractHash = await sha256File(path.resolve(contract))
  let appRoot = ''
  if (platform === 'windows') {
    const appExecutables = files.filter(entry => path.posix.basename(entry.path).toLowerCase() === 'depo-audio.exe')
    if (appExecutables.length !== 1) {
      throw new Error(`Expected exactly one packaged DepoAudio executable; found ${appExecutables.length}`)
    }
    const directory = path.posix.dirname(appExecutables[0].path)
    appRoot = directory === '.' ? '' : directory
  } else if (!files.some(entry => entry.path === 'Contents/MacOS/depo-audio')) {
    throw new Error('Packaged macOS app executable is missing from Contents/MacOS')
  }
  const resolvePackagedPath = value => {
    if (!value.startsWith('@app/')) throw new Error('Release binary contract packaged path is malformed')
    const suffix = value.slice('@app/'.length)
    return appRoot ? path.posix.join(appRoot, suffix) : suffix
  }

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

  const canonicalSpecs = new Map(
    stagedBinarySpecs(platform).map(([role, stagedPath, packagedBasename, packagedPath]) => [
      role,
      { stagedPath, packagedBasename, packagedPath },
    ]),
  )
  const expectedPackagedNames = new Set()
  for (const expected of binaryContract.files) {
    const canonical = canonicalSpecs.get(expected?.role)
    if (
      !canonical ||
      expected.stagedPath !== canonical.stagedPath ||
      expected.packagedBasename !== canonical.packagedBasename ||
      expected.packagedPath !== canonical.packagedPath ||
      !['byte-exact', 'packaged-smoke'].includes(expected.verification) ||
      !Number.isSafeInteger(expected.bytes) ||
      expected.bytes <= 0 ||
      !/^[0-9a-f]{64}$/u.test(expected.sha256)
    ) {
      throw new Error('Release binary contract contains a malformed file entry')
    }
    const packagedName = expected.packagedBasename.toLowerCase()
    if (expectedPackagedNames.has(packagedName)) {
      throw new Error(`Release binary contract repeats packaged name ${expected.packagedBasename}`)
    }
    expectedPackagedNames.add(packagedName)
    const matches = files.filter(entry => path.posix.basename(entry.path).toLowerCase() === packagedName)
    if (matches.length !== 1) {
      throw new Error(`Expected exactly one packaged ${expected.packagedBasename}; found ${matches.length}`)
    }
    const requiredPath = resolvePackagedPath(expected.packagedPath)
    if (matches[0].path !== requiredPath) {
      throw new Error(`Packaged ${expected.packagedBasename} is at ${matches[0].path}, expected ${requiredPath}`)
    }
    if (
      expected.verification === 'byte-exact' &&
      (matches[0].bytes !== expected.bytes || matches[0].sha256 !== expected.sha256)
    ) {
      throw new Error(`Packaged ${expected.packagedBasename} does not match its pre-package byte contract`)
    }
  }

  const packagedModels = files.filter(entry => entry.path.toLowerCase().endsWith('.onnx'))
  const expectedModels = binaryContract.files.filter(entry => entry.role.startsWith('model:'))
  if (
    packagedModels.length !== expectedModels.length ||
    expectedModels.some(entry => entry.verification !== 'byte-exact')
  ) {
    throw new Error(
      `Packaged ONNX inventory differs from the released model contract (${packagedModels.length} != ${expectedModels.length})`,
    )
  }

  const noticePaths = files.map(entry => entry.path.toLowerCase())
  const hasOrtLicense = noticePaths.some(
    item => item.includes('onnxruntime') && path.posix.basename(item) === 'license',
  )
  const hasOrtNotices = noticePaths.some(
    item => item.includes('onnxruntime') && path.posix.basename(item) === 'thirdpartynotices.txt',
  )
  const expectedRuntime = platform === 'macos' ? 'libonnxruntime.dylib' : 'onnxruntime.dll'
  const hasRuntime = noticePaths.some(item => path.posix.basename(item) === expectedRuntime)
  if (!hasOrtLicense || !hasOrtNotices || !hasRuntime) {
    throw new Error(
      `Missing ONNX Runtime distribution material (license=${hasOrtLicense}, notices=${hasOrtNotices}, runtime=${hasRuntime})`,
    )
  }

  const hasRustLicenseReport = noticePaths.some(
    item => item.includes('third-party/rust/') && path.posix.basename(item) === 'third-party-licenses.html',
  )
  if (!hasRustLicenseReport) {
    throw new Error('Missing generated Rust third-party license report')
  }

  const ffmpegNoticeRoot = 'third-party/ffmpeg/'
  const hasFfmpegSource = noticePaths.some(
    item => item.includes(ffmpegNoticeRoot) && path.posix.basename(item) === 'source.txt',
  )
  if (platform === 'windows') {
    const hasFfmpegLicense = noticePaths.some(
      item => item.includes(ffmpegNoticeRoot) && path.posix.basename(item) === 'license.txt',
    )
    const hasFfmpegConfiguration = noticePaths.some(
      item => item.includes(ffmpegNoticeRoot) && path.posix.basename(item) === 'build-configuration.txt',
    )
    if (!hasFfmpegLicense || !hasFfmpegConfiguration || !hasFfmpegSource) {
      throw new Error(
        `Missing FFmpeg distribution material (license=${hasFfmpegLicense}, configuration=${hasFfmpegConfiguration}, source=${hasFfmpegSource})`,
      )
    }
  } else {
    const requiredMacEvidence = [
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
    const missingMacEvidence = requiredMacEvidence.filter(
      expected => !noticePaths.some(item => item.includes(ffmpegNoticeRoot) && path.posix.basename(item) === expected),
    )
    if (!hasFfmpegSource || missingMacEvidence.length > 0) {
      throw new Error(
        `Missing macOS FFmpeg distribution material (source=${hasFfmpegSource}, missing=${missingMacEvidence.join(',')})`,
      )
    }
  }

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

async function selfTest() {
  const temporaryRoot = await mkdtemp(path.join(tmpdir(), 'depoaudio-inventory-test-'))
  try {
    const stagingRoot = path.join(temporaryRoot, 'staging')
    const artifactRoot = path.join(temporaryRoot, 'artifact')
    await mkdir(stagingRoot, { recursive: true })

    const noticeRoot = path.join(artifactRoot, 'resources', 'third-party', 'onnxruntime')
    await mkdir(noticeRoot, { recursive: true })
    await writeFile(path.join(noticeRoot, 'LICENSE'), 'license\n')
    await writeFile(path.join(noticeRoot, 'ThirdPartyNotices.txt'), 'notices\n')
    await writeFile(path.join(artifactRoot, 'depo-audio.exe'), 'application\n')
    const rustNoticeRoot = path.join(artifactRoot, 'resources', 'third-party', 'rust')
    await mkdir(rustNoticeRoot, { recursive: true })
    await writeFile(path.join(rustNoticeRoot, 'THIRD-PARTY-LICENSES.html'), 'rust licenses\n')
    const ffmpegNoticeRoot = path.join(artifactRoot, 'resources', 'third-party', 'ffmpeg')
    await mkdir(ffmpegNoticeRoot, { recursive: true })
    await writeFile(path.join(ffmpegNoticeRoot, 'LICENSE.txt'), 'license\n')
    await writeFile(path.join(ffmpegNoticeRoot, 'BUILD-CONFIGURATION.txt'), 'configuration\n')
    await writeFile(path.join(ffmpegNoticeRoot, 'SOURCE.txt'), 'source\n')

    const stagedFiles = [
      ['binaries/ffmpeg-x86_64-pc-windows-msvc.exe', 'ffmpeg.exe', 'ffmpeg\n'],
      ['binaries/ffprobe-x86_64-pc-windows-msvc.exe', 'ffprobe.exe', 'ffprobe\n'],
      ['resources/onnxruntime/onnxruntime.dll', 'resources/onnxruntime/onnxruntime.dll', 'runtime\n'],
      ...[...ACTIVE_MODEL_SHA256].map(([filename]) => [
        `resources/models/${filename}`,
        `resources/models/${filename}`,
        filename,
      ]),
    ]
    for (const [stagedPath, packagedPath, content] of stagedFiles) {
      const staged = path.join(stagingRoot, ...stagedPath.split('/'))
      const packaged = path.join(artifactRoot, ...packagedPath.split('/'))
      await mkdir(path.dirname(staged), { recursive: true })
      await mkdir(path.dirname(packaged), { recursive: true })
      await writeFile(staged, content)
      await writeFile(packaged, content)
    }

    // Use the real reviewed hashes for model fixture bytes without committing
    // test-sized fake models to a contract that claims those production hashes.
    const contractFiles = []
    for (const [role, stagedPath, packagedBasename, packagedPath] of stagedBinarySpecs('windows')) {
      const resolvedPackaged = path.join(artifactRoot, ...packagedPath.slice('@app/'.length).split('/'))
      const metadata = await lstat(resolvedPackaged)
      contractFiles.push({
        role,
        stagedPath,
        packagedBasename,
        packagedPath,
        verification: role.startsWith('model:') ? 'byte-exact' : 'packaged-smoke',
        bytes: metadata.size,
        sha256: await sha256File(resolvedPackaged),
      })
    }
    const contract = {
      schemaVersion: 1,
      platform: 'windows',
      sourceCommit: '0123456789abcdef0123456789abcdef01234567',
      releaseTag: 'v1.0.3-rc.test',
      files: contractFiles,
    }
    const contractPath = path.join(temporaryRoot, 'BINARY-CONTRACT-windows.json')
    const packagedContract = path.join(artifactRoot, 'resources', 'third-party', 'release', 'BINARY-CONTRACT.json')
    await mkdir(path.dirname(packagedContract), { recursive: true })
    const contractText = `${JSON.stringify(contract, null, 2)}\n`
    await writeFile(contractPath, contractText)
    await writeFile(packagedContract, contractText)

    const output = path.join(temporaryRoot, 'inventory.json')
    const inventory = await createInventory({ root: artifactRoot, output, platform: 'windows', contract: contractPath })
    if (inventory.fileCount !== 15) throw new Error('Self-test inventory count drifted')

    const packagedModel = path.join(artifactRoot, 'resources', 'models', 'flashsr.onnx')
    await writeFile(packagedModel, 'tampered model\n')
    let rejected = false
    try {
      await createInventory({ root: artifactRoot, output, platform: 'windows', contract: contractPath })
    } catch (error) {
      rejected = String(error).includes('does not match its pre-package byte contract')
    }
    if (!rejected) throw new Error('Self-test did not reject a packaged binary byte mismatch')
    await writeFile(packagedModel, 'flashsr.onnx')

    const misplacedModel = path.join(artifactRoot, 'flashsr.onnx')
    await rename(packagedModel, misplacedModel)
    rejected = false
    try {
      await createInventory({ root: artifactRoot, output, platform: 'windows', contract: contractPath })
    } catch (error) {
      rejected = String(error).includes('expected resources/models/flashsr.onnx')
    }
    if (!rejected) throw new Error('Self-test did not reject a model at the wrong packaged path')
    await rename(misplacedModel, packagedModel)

    await writeFile(path.join(artifactRoot, 'forbidden.trm'), 'forbidden fixture placeholder\n')
    rejected = false
    try {
      await createInventory({ root: artifactRoot, output, platform: 'windows', contract: contractPath })
    } catch (error) {
      rejected = String(error).includes('Forbidden FTR fixture material')
    }
    if (!rejected) throw new Error('Self-test did not reject a packaged TRM fixture')

    await rm(path.join(artifactRoot, 'forbidden.trm'))
    const retiredModel = path.join(artifactRoot, 'speaker_embed.onnx')
    await writeFile(retiredModel, 'retired model placeholder\n')
    rejected = false
    try {
      await createInventory({ root: artifactRoot, output, platform: 'windows', contract: contractPath })
    } catch (error) {
      rejected = String(error).includes('Retired model material')
    }
    if (!rejected) throw new Error('Self-test did not reject a retired model filename')
    if (
      !isRetiredModelEntry({
        path: 'renamed-model.bin',
        sha256: '7c5399d3da8a50ebef1c1a0ae421b33376aa5e45d0e92df16da7e83c9c131916',
      })
    ) {
      throw new Error('Self-test did not reject retired model bytes under a renamed path')
    }

    await rm(retiredModel)
    const macArtifactRoot = path.join(temporaryRoot, 'mac-app')
    const macResources = path.join(macArtifactRoot, 'Contents', 'Resources', 'resources')
    const macExecutableRoot = path.join(macArtifactRoot, 'Contents', 'MacOS')
    const macFrameworkRoot = path.join(macArtifactRoot, 'Contents', 'Frameworks')
    await mkdir(macExecutableRoot, { recursive: true })
    await mkdir(macFrameworkRoot, { recursive: true })
    await writeFile(path.join(macExecutableRoot, 'depo-audio'), 'application\n')
    await writeFile(path.join(macExecutableRoot, 'ffmpeg'), 'ffmpeg\n')
    await writeFile(path.join(macExecutableRoot, 'ffprobe'), 'ffprobe\n')
    await writeFile(path.join(macFrameworkRoot, 'libonnxruntime.dylib'), 'runtime\n')

    const macOrtNoticeRoot = path.join(macResources, 'third-party', 'onnxruntime')
    await mkdir(macOrtNoticeRoot, { recursive: true })
    await writeFile(path.join(macOrtNoticeRoot, 'LICENSE'), 'license\n')
    await writeFile(path.join(macOrtNoticeRoot, 'ThirdPartyNotices.txt'), 'notices\n')
    const macRustNoticeRoot = path.join(macResources, 'third-party', 'rust')
    await mkdir(macRustNoticeRoot, { recursive: true })
    await writeFile(path.join(macRustNoticeRoot, 'THIRD-PARTY-LICENSES.html'), 'rust licenses\n')
    const macFfmpegNoticeRoot = path.join(macResources, 'third-party', 'ffmpeg')
    await mkdir(macFfmpegNoticeRoot, { recursive: true })
    await writeFile(path.join(macFfmpegNoticeRoot, 'SOURCE.txt'), 'source\n')
    for (const filename of [
      'FFmpeg-LICENSE.md',
      'FFmpeg-COPYING.LGPLv2.1',
      'FFmpeg-COPYING.LGPLv3',
      'LAME-COPYING',
      'LAME-LICENSE',
      'Opus-COPYING',
      'BUILD-CONFIGURATION-arm64.txt',
      'BUILD-CONFIGURATION-x86_64.txt',
      'BUILD-TOOLCHAIN.txt',
    ]) {
      await writeFile(path.join(macFfmpegNoticeRoot, filename), 'evidence\n')
    }

    const macModelRoot = path.join(macResources, 'models')
    await mkdir(macModelRoot, { recursive: true })
    for (const [filename] of ACTIVE_MODEL_SHA256) {
      await writeFile(path.join(macModelRoot, filename), filename)
    }
    const macFiles = []
    for (const [role, stagedPath, packagedBasename, packagedPath] of stagedBinarySpecs('macos')) {
      const packaged = path.join(macArtifactRoot, ...packagedPath.slice('@app/'.length).split('/'))
      const metadata = await lstat(packaged)
      macFiles.push({
        role,
        stagedPath,
        packagedBasename,
        packagedPath,
        verification: role.startsWith('model:') ? 'byte-exact' : 'packaged-smoke',
        bytes: metadata.size,
        sha256: await sha256File(packaged),
      })
    }
    const macContract = { ...contract, platform: 'macos', files: macFiles }
    const macContractPath = path.join(temporaryRoot, 'BINARY-CONTRACT-macos.json')
    const macContractText = `${JSON.stringify(macContract, null, 2)}\n`
    await writeFile(macContractPath, macContractText)
    const macPackagedContract = path.join(macResources, 'third-party', 'release', 'BINARY-CONTRACT.json')
    await mkdir(path.dirname(macPackagedContract), { recursive: true })
    await writeFile(macPackagedContract, macContractText)
    await createInventory({ root: macArtifactRoot, output, platform: 'macos', contract: macContractPath })
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
