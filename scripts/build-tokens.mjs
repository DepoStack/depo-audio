#!/usr/bin/env node
// Generate src/styles/tokens.css from design/tokens.json (DTCG format).
//
//   npm run tokens
//
// The JSON is the source of truth for every theme CSS variable; the generated
// file is committed so builds don't depend on this script, and CI regenerates
// it to fail on drift. Resolution and output formats live in ./lib/tokens.mjs.

import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createResolver, cssValue } from './lib/tokens.mjs'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const tokens = JSON.parse(readFileSync(join(root, 'design', 'tokens.json'), 'utf8'))
const resolver = createResolver(tokens)

// Flatten a group into [cssName, token] pairs: nested groups join with '-'.
function flatten(group, prefix = '') {
  const out = []
  for (const [name, token] of Object.entries(group)) {
    if (name.startsWith('$')) continue
    const full = prefix ? `${prefix}-${name}` : name
    if (token && typeof token === 'object' && '$value' in token) out.push([full, token])
    else out.push(...flatten(token, full))
  }
  return out
}

function emitBlock(selector, group, prefix = '') {
  const lines = [`  ${selector} {`]
  for (const [name, token] of flatten(group)) {
    if (token.$description) lines.push(`    /* ${token.$description} */`)
    lines.push(`    --${prefix}${name}: ${cssValue(name, token, resolver)};`)
  }
  lines.push('  }')
  return lines.join('\n')
}

const css = `/* GENERATED FILE — do not edit by hand.
   Source: design/tokens.json · regenerate with \`npm run tokens\` */

@layer base {
${emitBlock(':root', tokens.base, 'ds-')}

${emitBlock(':root', tokens.semantic.dark)}

${emitBlock('.light', tokens.semantic.light)}
}
`

const out = join(root, 'src', 'styles', 'tokens.css')
mkdirSync(dirname(out), { recursive: true })
writeFileSync(out, css)
console.log(`wrote ${out}`)
