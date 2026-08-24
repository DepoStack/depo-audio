import { readdirSync, statSync } from 'node:fs'
import { extname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const distDirectory = fileURLToPath(new URL('../dist/', import.meta.url))
const budgets = {
  javascript: 560 * 1024,
  css: 64 * 1024,
}

const emittedFiles = []

function collectFiles(directory) {
  const entries = readdirSync(directory, { withFileTypes: true }).sort((left, right) =>
    left.name.localeCompare(right.name),
  )

  for (const entry of entries) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) collectFiles(path)
    if (entry.isFile()) {
      emittedFiles.push({
        path: relative(distDirectory, path),
        extension: extname(entry.name).toLowerCase(),
        size: statSync(path).size,
      })
    }
  }
}

try {
  collectFiles(distDirectory)
} catch (error) {
  if (error?.code === 'ENOENT') {
    console.error('Client size check failed: dist/ does not exist. Run npm run build first.')
    process.exit(1)
  }
  throw error
}

const assets = {
  javascript: emittedFiles.filter(({ extension }) => extension === '.js' || extension === '.mjs'),
  css: emittedFiles.filter(({ extension }) => extension === '.css'),
}

const formatKiB = bytes => `${(bytes / 1024).toFixed(1)} KiB`
const failures = []

for (const [type, typeFiles] of Object.entries(assets)) {
  if (typeFiles.length === 0) {
    failures.push(`No emitted ${type} files were found in dist/.`)
    continue
  }

  const total = typeFiles.reduce((sum, file) => sum + file.size, 0)
  const budget = budgets[type]
  console.log(`${type}: ${formatKiB(total)} / ${formatKiB(budget)} (${typeFiles.length} files)`)

  if (total > budget) {
    failures.push(`${type} exceeds its budget by ${formatKiB(total - budget)}.`)
  }
}

if (failures.length > 0) {
  console.error('Client size check failed:')
  for (const failure of failures) console.error(`- ${failure}`)
  process.exit(1)
}

console.log('Client size budgets passed.')
