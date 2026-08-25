#!/usr/bin/env node

import { basename } from 'node:path'
import { readFile, stat } from 'node:fs/promises'

const API_VERSION = '2022-11-28'

function fail(message) {
  throw new Error(message)
}

function parseArguments(argv) {
  const separator = argv.indexOf('--')
  const options = separator < 0 ? argv : argv.slice(0, separator)
  const files = separator < 0 ? [] : argv.slice(separator + 1)
  const value = flag => {
    const index = options.indexOf(flag)
    if (index < 0 || !options[index + 1]) fail(`Missing ${flag}`)
    return options[index + 1]
  }
  return {
    repo: value('--repo'),
    releaseId: value('--release-id'),
    tag: value('--tag'),
    commit: value('--commit'),
    files,
  }
}

function validateContract(contract) {
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(contract.repo)) fail('Invalid repository contract')
  if (!/^[1-9][0-9]*$/.test(String(contract.releaseId))) fail('Invalid release ID contract')
  if (!/^v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z][0-9A-Za-z.-]*)?$/.test(contract.tag)) {
    fail('Invalid release tag contract')
  }
  if (!/^[0-9a-f]{40}$/.test(contract.commit)) fail('Invalid release commit contract')
  if (contract.files.length === 0) fail('No release assets were supplied')
  const names = contract.files.map(file => basename(file))
  if (new Set(names).size !== names.length) fail('Release asset basenames must be unique')
  if (names.some(name => name === '.' || name === '..' || /[\x00-\x1f\x7f/\\]/.test(name))) {
    fail('Unsafe release asset name')
  }
}

function assertDraftRelease(release, contract, expectedNames = []) {
  if (
    release?.id !== Number(contract.releaseId) ||
    release?.draft !== true ||
    release?.tag_name !== contract.tag ||
    release?.target_commitish !== contract.commit
  ) {
    fail('The exact commit-bound draft release contract no longer matches')
  }
  const existing = new Set((release.assets ?? []).map(asset => asset.name))
  for (const name of expectedNames) {
    if (existing.has(name)) fail(`Draft release already contains asset ${name}`)
  }
}

async function githubRequest(url, token, init = {}) {
  const response = await fetch(url, {
    ...init,
    headers: {
      Accept: 'application/vnd.github+json',
      Authorization: `Bearer ${token}`,
      'X-GitHub-Api-Version': API_VERSION,
      ...(init.headers ?? {}),
    },
  })
  if (!response.ok) {
    const body = (await response.text()).slice(0, 500)
    fail(`GitHub API request failed with HTTP ${response.status}: ${body}`)
  }
  return response
}

async function getRelease(contract, token) {
  const response = await githubRequest(
    `https://api.github.com/repos/${contract.repo}/releases/${contract.releaseId}`,
    token,
  )
  return response.json()
}

async function uploadAsset(contract, token, file) {
  const name = basename(file)
  const metadata = await stat(file)
  if (!metadata.isFile() || metadata.size <= 0) fail(`Release asset ${name} is missing or empty`)

  const before = await getRelease(contract, token)
  assertDraftRelease(before, contract, [name])
  const body = await readFile(file)
  const response = await githubRequest(
    `https://uploads.github.com/repos/${contract.repo}/releases/${contract.releaseId}/assets?name=${encodeURIComponent(name)}`,
    token,
    {
      method: 'POST',
      headers: {
        'Content-Type': 'application/octet-stream',
        'Content-Length': String(body.byteLength),
      },
      body,
    },
  )
  const uploaded = await response.json()
  if (uploaded?.name !== name || uploaded?.size !== body.byteLength || uploaded?.state !== 'uploaded') {
    fail(`GitHub did not confirm the uploaded asset contract for ${name}`)
  }

  const after = await getRelease(contract, token)
  assertDraftRelease(after, contract)
  const matches = (after.assets ?? []).filter(
    asset =>
      asset.name === name && asset.id === uploaded.id && asset.size === body.byteLength && asset.state === 'uploaded',
  )
  if (matches.length !== 1) fail(`Uploaded asset ${name} was not preserved exactly once on the draft`)
  process.stdout.write(`Uploaded ${name} to verified draft release ${contract.releaseId}\n`)
}

function selfTest() {
  const contract = {
    repo: 'DepoStack/depo-audio',
    releaseId: '123',
    tag: 'v1.0.3-rc.2',
    commit: 'a'.repeat(40),
    files: ['one.txt', 'two.txt'],
  }
  validateContract(contract)
  const validRelease = {
    id: 123,
    draft: true,
    tag_name: contract.tag,
    target_commitish: contract.commit,
    assets: [],
  }
  assertDraftRelease(validRelease, contract, ['one.txt'])

  for (const [label, release, expectedNames] of [
    ['published release', { ...validRelease, draft: false }, []],
    ['wrong release ID', { ...validRelease, id: 124 }, []],
    ['wrong tag', { ...validRelease, tag_name: 'v1.0.4-rc.1' }, []],
    ['wrong commit', { ...validRelease, target_commitish: 'b'.repeat(40) }, []],
    ['existing asset collision', { ...validRelease, assets: [{ name: 'one.txt' }] }, ['one.txt']],
  ]) {
    let rejected = false
    try {
      assertDraftRelease(release, contract, expectedNames)
    } catch {
      rejected = true
    }
    if (!rejected) fail(`Draft release upload self-test accepted ${label}`)
  }
  for (const invalid of [
    { ...contract, releaseId: '0' },
    { ...contract, tag: 'latest' },
    { ...contract, commit: 'main' },
    { ...contract, files: ['same', 'same'] },
  ]) {
    let rejected = false
    try {
      validateContract(invalid)
    } catch {
      rejected = true
    }
    if (!rejected) fail('Draft release upload self-test accepted an invalid contract')
  }
  process.stdout.write('Draft release upload self-test passed\n')
}

if (process.argv.includes('--self-test')) {
  selfTest()
} else {
  const contract = parseArguments(process.argv.slice(2))
  validateContract(contract)
  const token = process.env.GH_TOKEN
  if (!token) fail('GH_TOKEN is required')
  for (const file of contract.files) await uploadAsset(contract, token, file)
}
