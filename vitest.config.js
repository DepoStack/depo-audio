import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'
import { sharedAliases } from './config/vite-aliases.js'

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: sharedAliases,
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/__tests__/setup.js'],
    include: ['src/**/*.test.{js,jsx}'],
  },
})
