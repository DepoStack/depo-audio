/* eslint-disable react-refresh/only-export-components -- provider and matching hook intentionally share one context */
import { createContext, useContext } from 'react'
import usePreferences from './usePreferences'

const PreferencesContext = createContext(null)

export function PreferencesProvider({ children }) {
  const prefs = usePreferences()
  return <PreferencesContext.Provider value={prefs}>{children}</PreferencesContext.Provider>
}

export function usePreferencesContext() {
  const ctx = useContext(PreferencesContext)
  if (!ctx) throw new Error('usePreferencesContext must be used within PreferencesProvider')
  return ctx
}
