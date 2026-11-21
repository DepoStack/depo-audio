import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
import process from 'node:process'
import postcss from 'postcss'
import tailwind from '@tailwindcss/postcss'

// Builds the app's real stylesheet (src/globals.css through Tailwind, the same
// plugin the app build uses) so a jsdom test can read computed styles instead
// of class names. jsdom does not lay anything out, but it does cascade
// stylesheets into getComputedStyle. It ignores rules inside @layer, so the
// layers are unwrapped in their source order, which keeps utilities last.
export async function buildAppStylesheet() {
  // vitest runs from the repository root (package.json scripts)
  const from = resolve(process.cwd(), 'src/globals.css')
  const result = await postcss([tailwind()]).process(await readFile(from, 'utf8'), { from })
  const root = postcss.parse(result.css)
  root.walkAtRules('layer', rule => {
    if (rule.nodes) rule.replaceWith(rule.nodes)
    else rule.remove()
  })
  return root.toString()
}

export function injectStylesheet(css) {
  const style = document.createElement('style')
  style.setAttribute('data-test-stylesheet', 'app')
  style.textContent = css
  document.head.appendChild(style)
  return () => style.remove()
}
