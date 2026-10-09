import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const repository = fileURLToPath(new URL('../', import.meta.url))

function publicationFixture(t) {
  const directory = mkdtempSync(join(tmpdir(), 'depoaudio-release-contract-test-'))
  t.after(() => rmSync(directory, { recursive: true, force: true }))
  const root = join(directory, 'repository')
  cpSync(repository, root, {
    recursive: true,
    filter: source => {
      const parts = relative(repository, source).split(sep)
      return (
        !['.git', '.agents', '.codex', 'node_modules', 'dist'].includes(parts[0]) &&
        !(parts[0] === 'src-tauri' && ['target', 'binaries', 'gen'].includes(parts[1]))
      )
    },
  })
  const gatePath = join(root, 'docs/V1.0.3-RELEASE-GATE.json')
  const gate = JSON.parse(readFileSync(gatePath, 'utf8'))
  const reviewedDate = gate.reviewedDate
  gate.status = 'GO'
  Object.assign(gate.candidate, {
    updater: 'unavailable',
    updaterPublicKey: null,
    publicationDate: reviewedDate,
    approvedBy: 'Synthetic fixture reviewer',
  })
  for (const blocker of gate.blockers) {
    Object.assign(blocker, {
      status: 'closed',
      closedDate: reviewedDate,
      closedBy: 'Synthetic fixture reviewer',
      evidenceRefs: blocker.evidenceRefs ?? ['synthetic fixture; not release approval'],
    })
  }
  const candidatePath = join(root, 'docs/V1.0.3-RELEASE-CANDIDATE.md')
  writeFileSync(
    candidatePath,
    readFileSync(candidatePath, 'utf8').replace(
      'Status: **NO-GO for publication; private RC evidence only**',
      'Status: **GO for publication**',
    ),
  )
  const changelogPath = join(root, 'CHANGELOG.md')
  writeFileSync(
    changelogPath,
    readFileSync(changelogPath, 'utf8').replace(
      `## [${gate.version}] - Unpublished`,
      `## [${gate.version}] - ${reviewedDate}`,
    ),
  )
  return {
    gate,
    run(now = `${reviewedDate}T12:00:00Z`) {
      writeFileSync(gatePath, JSON.stringify(gate, null, 2))
      // Only the child fixture's clock is fixed. Production expiry remains enforced.
      const clock = join(directory, 'clock.cjs')
      writeFileSync(clock, `Date.now = () => Date.parse(${JSON.stringify(now)})\n`)
      return spawnSync(process.execPath, ['--require', clock, 'scripts/release-contract-check.mjs', '--publication-ready'], {
        cwd: root,
        encoding: 'utf8',
      })
    },
  }
}

test('publication-ready accepts all blockers closed with reviewers and retained candidate evidence', t => {
  const result = publicationFixture(t).run()
  assert.equal(result.status, 0, result.stderr)
  assert.match(result.stdout, /GO; 0 open publication blockers/)
})

test('publication-ready rejects a closed technical blocker without an accountable reviewer', t => {
  const fixture = publicationFixture(t)
  delete fixture.gate.blockers.find(blocker => blocker.id === 'javascript-runtime-notices').closedBy
  const result = fixture.run()
  assert.equal(result.status, 1)
  assert.match(result.stderr, /javascript-runtime-notices must identify the accountable reviewer/)
})

test('publication-ready rejects closed technical evidence that loses the exact candidate binding', t => {
  const fixture = publicationFixture(t)
  fixture.gate.blockers.find(blocker => blocker.id === 'macos-ffmpeg-rc2-evidence').evidenceRefs = ['unrelated evidence']
  const result = fixture.run()
  assert.equal(result.status, 1)
  assert.match(result.stderr, /macos-ffmpeg-rc2-evidence must retain exact candidate evidence/)
})

test('publication-ready still rejects an open blocker', t => {
  const fixture = publicationFixture(t)
  fixture.gate.blockers.find(blocker => blocker.id === 'synthetic-ftr-packaged-decode').status = 'open'
  const result = fixture.run()
  assert.equal(result.status, 1)
  assert.match(result.stderr, /every machine-readable release blocker must be closed before publication/)
})

test('publication-ready still rejects the expired advisory exception after valid blocker closure', t => {
  const result = publicationFixture(t).run('2026-10-01T00:00:00Z')
  assert.equal(result.status, 1)
  assert.match(result.stderr, /rust-unic maintenance exception must have a current named owner and unexpired review date/)
})
