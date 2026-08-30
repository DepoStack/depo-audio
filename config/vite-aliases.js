import { fileURLToPath } from 'node:url'

const scrollLockCompat = fileURLToPath(new URL('../src/lib/scroll-lock-compat.jsx', import.meta.url))

export const sharedAliases = [
  { find: /^react-remove-scroll-bar\/constants$/, replacement: scrollLockCompat },
  { find: /^react-remove-scroll-bar$/, replacement: scrollLockCompat },
]
