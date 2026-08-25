import assert from 'node:assert/strict'
import { readFile, writeFile } from 'node:fs/promises'

const normalize = source => {
  const lineNormalized = source.replaceAll('\r\n', '\n')
  if (lineNormalized.includes('\r')) {
    throw new Error('cargo-about report contains an unsupported bare carriage return')
  }
  return lineNormalized.replace(/[ \t]+(?=\n|$)/gu, '')
}

const args = process.argv.slice(2)
if (args.includes('--self-test')) {
  assert.equal(normalize('alpha  \r\nbeta\t\n'), 'alpha\nbeta\n')
  assert.throws(() => normalize('alpha\rbeta'), /bare carriage return/u)
  console.log('cargo-about report normalizer self-test passed')
  process.exit(0)
}

const valueAfter = flag => {
  const index = args.indexOf(flag)
  if (index < 0 || !args[index + 1]) throw new Error(`Missing required ${flag} value`)
  return args[index + 1]
}

const input = valueAfter('--input')
const output = valueAfter('--output')
const source = await readFile(input, 'utf8')
await writeFile(output, normalize(source), 'utf8')
console.log('Normalized cargo-about report line endings to LF')
