import { useCallback, useEffect } from 'react'
import { useTheme as useNextTheme } from 'next-themes'

const VALID_THEMES = new Set(['system', 'dark', 'light'])

export default function useTheme(preferenceTheme, setPreferenceTheme) {
  const { resolvedTheme, setTheme: setNextTheme } = useNextTheme()
  const themePref = VALID_THEMES.has(preferenceTheme) ? preferenceTheme : 'system'

  // next-themes owns the DOM class, while usePreferences owns persistence.
  // Reconcile the DOM whenever the backend-hydrated preference changes.
  useEffect(() => {
    setNextTheme(themePref)
  }, [themePref, setNextTheme])

  const cycleTheme = useCallback(() => {
    const next = themePref === 'system' ? 'dark' : themePref === 'dark' ? 'light' : 'system'
    setNextTheme(next)
    setPreferenceTheme(next)
  }, [themePref, setNextTheme, setPreferenceTheme])

  const setThemeDirect = useCallback(
    value => {
      const next = VALID_THEMES.has(value) ? value : 'system'
      setNextTheme(next)
      setPreferenceTheme(next)
    },
    [setNextTheme, setPreferenceTheme],
  )

  return {
    theme: resolvedTheme || (themePref === 'system' ? 'dark' : themePref),
    themePref,
    themeLabel: themePref,
    cycleTheme,
    setThemeDirect,
  }
}
