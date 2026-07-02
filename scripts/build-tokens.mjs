#!/usr/bin/env node
// Generate src/styles/tokens.css from design/tokens.json (DTCG format).
//
//   npm run tokens
//
// The JSON is the source of truth for every theme CSS variable; the generated
// file is committed so builds don't depend on this script, and CI regenerates
// it to fail on drift. Alias tokens reference primitives as {dot.path} —
// references may be embedded in longer strings (e.g. "{color.gold.500} / 0.14").

import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const tokens = JSON.parse(readFileSync(join(root, 'design', 'tokens.json'), 'utf8'))

function lookup(path) {
  let node = tokens
  for (const key of path.split('.')) {
    node = node?.[key]
    if (node === undefined) throw new Error(`Unknown token reference: {${path}}`)
  }
  if (typeof node?.$value !== 'string') throw new Error(`Reference {${path}} does not resolve to a token with a string $value`)
  return resolve(node.$value)
}

function resolve(value) {
  return value.replace(/\{([^}]+)\}/g, (_, path) => lookup(path))
}

function emitBlock(selector, group) {
  const lines = [`  ${selector} {`]
  for (const [name, token] of Object.entries(group)) {
    if (name.startsWith('$')) continue
    if (token.$description) lines.push(`    /* ${token.$description} */`)
    lines.push(`    --${name}: ${resolve(token.$value)};`)
  }
  lines.push('  }')
  return lines.join('\n')
}

const css = `/* GENERATED FILE — do not edit by hand.
   Source: design/tokens.json · regenerate with \`npm run tokens\` */

@layer base {
${emitBlock(':root', tokens.semantic.dark)}

${emitBlock('.light', tokens.semantic.light)}
}
`

const out = join(root, 'src', 'styles', 'tokens.css')
mkdirSync(dirname(out), { recursive: true })
writeFileSync(out, css)
console.log(`wrote ${out}`)
