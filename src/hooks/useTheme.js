import { useCallback } from 'react'
import { useTheme as useNextTheme } from 'next-themes'
import { invoke } from '@tauri-apps/api/core'

export default function useTheme() {
  const { theme, resolvedTheme, setTheme: setNextTheme } = useNextTheme()

  const cycleTheme = useCallback(() => {
    const next = theme === 'system' ? 'dark' : theme === 'dark' ? 'light' : 'system'
    setNextTheme(next)
    invoke('prefs_set', { patch: { theme: next } }).catch(() => {})
  }, [theme, setNextTheme])

  const setThemeDirect = useCallback((value) => {
    setNextTheme(value)
    invoke('prefs_set', { patch: { theme: value } }).catch(() => {})
  }, [setNextTheme])

  const themeLabel = theme === 'system' ? 'system' : theme === 'dark' ? 'dark' : 'light'

  return { theme: resolvedTheme || 'dark', themePref: theme, themeLabel, cycleTheme, setThemeDirect }
}
