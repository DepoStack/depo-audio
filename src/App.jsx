import { useState, useEffect, useRef, useCallback, lazy, Suspense } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'
import { Sun, Moon, Monitor, Settings, AudioLines, Play, FolderOpen } from 'lucide-react'
import { DEPOSTACK_URL } from './constants'
import { shouldIgnoreNavigationShortcut } from './lib/utils'

import useTheme from './hooks/useTheme'
import { usePreferencesContext } from './hooks/PreferencesContext'
import useFileDrop from './hooks/useFileDrop'
import useConversion from './hooks/useConversion'
import useUpdater from './hooks/useUpdater'
import UpdateBanner from './components/UpdateBanner'

import { LogoSvg } from './components/common/Icons'
import Spinner from './components/common/Spinner'
import { Tabs, TabsList, TabsTrigger, TabsContent } from './components/ui/tabs'
import { Badge } from './components/ui/badge'
import { Button } from './components/ui/button'
import ConvertTab from './components/Convert/ConvertTab'

const LibraryTab = lazy(() => import('./components/Library/LibraryTab'))
const PlayerTab = lazy(() => import('./components/Player/PlayerTab'))
// Merge is hidden for v1 — MergeTab.jsx is retained; re-add the nav entry and
// TabsContent below to bring it back (see the "Re-add Merge" tracking issue).
const SettingsPanel = lazy(() => import('./components/SettingsPanel'))

const themeIcons = {
  light: Sun,
  dark: Moon,
  system: Monitor,
}

// Sidebar navigation: tab id → icon, label, and its number-key shortcut
const NAV = [
  { id: 'convert', label: 'Convert', Icon: AudioLines },
  { id: 'player', label: 'Player', Icon: Play },
  { id: 'library', label: 'Library', Icon: FolderOpen },
]

export default function App() {
  const [tab, setTab] = useState('convert')

  // Custom hooks
  const prefs = usePreferencesContext()
  const { labels, outDir, prefsReady, prefsError } = prefs
  const { themePref, themeLabel, cycleTheme, setThemeDirect } = useTheme(prefs.themePref, prefs.setThemePref)
  const [settingsOpen, setSettingsOpen] = useState(false)

  // Auto-update via GitHub Releases (checks once on launch)
  const updater = useUpdater()

  // While the Player tab is mounted it registers its own drop handler here,
  // so native drops land in the playlist instead of the convert queue
  const dropOverrideRef = useRef(null)
  const conversion = useConversion()
  const { jobs, converting, cancelling, cancelError, doneCount, failCount, cancelledCount } = conversion
  const queueLocked = converting || !prefsReady
  const fileDrop = useFileDrop(dropOverrideRef, queueLocked)
  const {
    files,
    dragOver,
    caseName,
    setCaseName,
    queueNotice,
    addFiles,
    onDragOver,
    onDragLeave,
    onDrop,
    browseFiles,
    browseOutDir,
    removeFile,
    clearAll,
  } = fileDrop

  // System capabilities (hardware-aware recommendations)
  const [capabilities, setCapabilities] = useState(null)
  const [systemError, setSystemError] = useState('')
  useEffect(() => {
    invoke('system_capabilities_cmd')
      .then(setCapabilities)
      .catch(error => setSystemError(`System capabilities could not be loaded: ${String(error)}`))
  }, [])

  // Sidebar health card: sidecars + installed AI models
  const [health, setHealth] = useState(null)
  useEffect(() => {
    invoke('health_check')
      .then(setHealth)
      .catch(error => setSystemError(`Audio engine health could not be checked: ${String(error)}`))
  }, [])

  // Library state
  const [cases, setCases] = useState([])
  const [libSearch, setLibSearch] = useState('')
  const [libraryReadError, setLibraryReadError] = useState('')
  const [libraryOperationError, setLibraryOperationError] = useState('')
  const reportLibraryError = useCallback(error => {
    setLibraryOperationError(`${String(error)} Existing Library data was left unchanged.`)
  }, [])
  const loadLibrary = useCallback(() => {
    invoke('library_get')
      .then(value => {
        setCases(value)
        setLibraryReadError('')
        setLibraryOperationError('')
      })
      .catch(error => {
        setLibraryReadError(
          `Library storage could not be loaded: ${String(error)}. Library changes are blocked to protect the existing data file.`,
        )
      })
  }, [])

  // Load library on startup (nav badge count) and when opening the tab
  useEffect(() => {
    loadLibrary()
  }, [loadLibrary])
  useEffect(() => {
    if (tab === 'library') loadLibrary()
  }, [tab, loadLibrary])

  // Number keys switch tabs (ignored while typing or with modifiers held)
  useEffect(() => {
    const onKey = e => {
      if (shouldIgnoreNavigationShortcut(e)) return
      const idx = ['1', '2', '3', '4'].indexOf(e.key)
      if (idx >= 0 && idx < NAV.length) setTab(NAV[idx].id)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const handleStartConversion = () => {
    if (!prefsReady || files.some(file => file.fmt?.status === 'unsupported')) return
    conversion.startConversion({
      files,
      outDir,
      ...prefs,
      autoLevel: prefs.mode !== 'keep' && prefs.autoLevel,
      dereverb: Boolean(prefs.dereverb && capabilities?.dereverbAvailable),
      caseName,
      setCases,
      onLibraryError: reportLibraryError,
    })
  }

  const handleQueueFromLibrary = async (sourcePaths, sourceCaseName) => {
    const paths = Array.isArray(sourcePaths) ? sourcePaths : [sourcePaths]
    const result = await addFiles(paths, { replace: true })
    if (result?.locked) return false
    setCaseName(sourceCaseName || '')
    setTab('convert')
    return true
  }

  const ThemeIcon = themeIcons[themeLabel] || Monitor
  const libCount = cases.filter(c => !c.archived).length
  const modelCount = health?.models?.length ?? null
  const updaterLabel = updater.error
    ? 'Update check unavailable'
    : updater.status === 'available'
      ? 'Update available'
      : updater.status === 'checking'
        ? 'Checking updates…'
        : updater.status === 'downloading' || updater.status === 'ready'
          ? 'Installing update…'
          : 'Up to date'

  return (
    <Tabs value={tab} onValueChange={setTab} orientation="vertical" className="flex h-screen overflow-hidden">
      {/* ── Sidebar ── */}
      <aside
        className="w-16 md:w-56 shrink-0 flex flex-col bg-card border-r border-border select-none"
        data-tauri-drag-region
      >
        <div className="flex items-center gap-2.5 px-3 md:px-4 pt-4 pb-5">
          <LogoSvg />
          <div className="hidden md:flex flex-col leading-none">
            <span className="font-serif text-[16px] font-semibold text-foreground">DepoAudio</span>
            <span className="text-[9.5px] text-[hsl(var(--sub))] tracking-wider">Audio Converter &amp; Enhancer</span>
          </div>
        </div>

        <TabsList
          aria-label="Main navigation"
          className="flex-col items-stretch gap-1 bg-transparent border-none rounded-none p-0 px-2 md:px-3 h-auto"
        >
          {NAV.map(({ id, label, Icon }, i) => (
            <TabsTrigger
              key={id}
              value={id}
              aria-label={label}
              className="w-full justify-start gap-2.5 px-2.5 md:px-3 py-2 rounded-lg"
            >
              <Icon size={16} aria-hidden="true" className="shrink-0" />
              <span className="hidden md:inline">{label}</span>
              {id === 'library' && libCount > 0 && (
                <Badge variant="gold" className="hidden md:inline-flex">
                  {libCount}
                </Badge>
              )}
              <kbd
                aria-hidden="true"
                className="hidden md:inline ml-auto font-mono text-[9.5px] px-1.5 py-px rounded border border-border/70 bg-secondary/60 text-[hsl(var(--sub))]"
              >
                {i + 1}
              </kbd>
            </TabsTrigger>
          ))}
        </TabsList>

        <div className="flex-1" />

        {/* System health */}
        <div className="hidden md:block mx-3 mb-2 px-3 py-2.5 rounded-lg border border-border/70 bg-[hsl(var(--surface))]">
          <div className="flex items-center gap-2 py-0.5 text-[11px] font-medium text-[hsl(var(--text2))]">
            <span
              aria-hidden="true"
              className={`w-1.5 h-1.5 rounded-full ${health ? (health.ffmpeg ? 'bg-[hsl(var(--success))]' : 'bg-destructive') : 'bg-[hsl(var(--sub))]'}`}
            />
            {health ? (health.ffmpeg ? 'FFmpeg ready' : 'FFmpeg missing') : 'Checking engine…'}
          </div>
          {modelCount != null && (
            <div className="flex items-center gap-2 py-0.5 text-[11px] font-medium text-[hsl(var(--text2))]">
              <span
                aria-hidden="true"
                className={`w-1.5 h-1.5 rounded-full ${modelCount > 0 ? 'bg-[hsl(var(--success))]' : 'bg-[hsl(var(--warning))]'}`}
              />
              {modelCount} AI model{modelCount !== 1 ? 's' : ''} installed
            </div>
          )}
          <div className="py-0.5 text-[11px] text-[hsl(var(--sub))]">{updaterLabel}</div>
        </div>

        {/* DepoStack brand link — DepoAudio is one tool in the suite */}
        <button
          type="button"
          onClick={() =>
            openUrl(DEPOSTACK_URL).catch(error => setSystemError(`Could not open DepoStack: ${String(error)}`))
          }
          title="Part of the DepoStack suite — opens depostack.com"
          className="hidden md:block mx-3 mb-1.5 px-3 text-left text-[10.5px] text-[hsl(var(--sub))] hover:text-[hsl(var(--gold))] transition-colors"
        >
          A <span className="font-semibold text-[hsl(var(--text2))]">DepoStack</span> project&nbsp;↗
        </button>

        <div className="flex md:justify-start justify-center items-center gap-1 px-2 md:px-3 pb-3">
          <Button
            variant="ghost"
            size="icon"
            title={prefsError ? 'Settings disabled because preference storage could not be loaded' : 'Settings'}
            aria-label="Settings"
            disabled={!prefsReady || !!prefsError}
            onClick={() => setSettingsOpen(true)}
          >
            <Settings size={16} aria-hidden="true" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            title={
              prefsError ? 'Theme changes disabled because preference storage is read-only' : `Theme: ${themePref}`
            }
            aria-label={`Switch theme (current: ${themePref})`}
            disabled={!!prefsError}
            onClick={cycleTheme}
          >
            <ThemeIcon size={16} aria-hidden="true" />
          </Button>
        </div>
      </aside>

      {/* ── Main column ── */}
      <div className="flex-1 flex flex-col min-w-0 overflow-hidden">
        <UpdateBanner updater={updater} />

        {(prefsError || libraryReadError) && (
          <div
            role="alert"
            className="mx-5 md:mx-8 mt-3 rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-[11px] text-foreground"
          >
            <span className="font-semibold">Storage protection active.</span>{' '}
            {[prefsError, libraryReadError].filter(Boolean).join(' ')}
          </div>
        )}

        {libraryOperationError && (
          <div
            role="alert"
            className="mx-5 md:mx-8 mt-3 rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 text-[11px] text-foreground"
          >
            <span className="font-semibold">Library change not saved.</span> {libraryOperationError}
          </div>
        )}

        {systemError && (
          <div
            role="alert"
            className="mx-5 md:mx-8 mt-3 flex items-start gap-2 rounded-lg border border-warning/40 bg-warning/10 px-3 py-2 text-[11px] text-foreground"
          >
            <span className="flex-1">{systemError}</span>
            <button type="button" aria-label="Dismiss system warning" onClick={() => setSystemError('')}>
              ×
            </button>
          </div>
        )}

        {!prefsReady && (
          <div className="flex flex-1 items-center justify-center gap-2 text-sm text-[hsl(var(--sub))]">
            <Spinner className="h-5 w-5" /> Loading preferences…
          </div>
        )}

        {prefsReady && (
          <>
            <TabsContent value="convert" forceMount={tab === 'convert' ? true : undefined}>
              {tab === 'convert' && (
                <ConvertTab
                  capabilities={capabilities}
                  files={files}
                  dragOver={dragOver}
                  caseName={caseName}
                  setCaseName={setCaseName}
                  onDragOver={onDragOver}
                  onDragLeave={onDragLeave}
                  onDrop={onDrop}
                  browseFiles={browseFiles}
                  browseOutDir={browseOutDir}
                  removeFile={removeFile}
                  clearAll={clearAll}
                  jobs={jobs}
                  converting={converting}
                  cancelling={cancelling}
                  conversionError={cancelError}
                  queueLocked={queueLocked}
                  queueNotice={queueNotice}
                  startConversion={handleStartConversion}
                  cancelConversion={conversion.cancelConversion}
                  doneCount={doneCount}
                  failCount={failCount}
                  cancelledCount={cancelledCount}
                />
              )}
            </TabsContent>

            <TabsContent value="player">
              <Suspense
                fallback={
                  <div className="flex items-center justify-center py-12">
                    <Spinner className="h-5 w-5" />
                  </div>
                }
              >
                <PlayerTab
                  dropHandlerRef={dropOverrideRef}
                  onConvertFiles={async paths => {
                    const result = await addFiles(paths)
                    if (!result?.locked) setTab('convert')
                    return !result?.locked
                  }}
                />
              </Suspense>
            </TabsContent>

            <TabsContent value="library">
              <Suspense
                fallback={
                  <div className="flex items-center justify-center py-12">
                    <Spinner className="h-5 w-5" />
                  </div>
                }
              >
                <LibraryTab
                  cases={cases}
                  setCases={setCases}
                  search={libSearch}
                  setSearch={setLibSearch}
                  labels={labels}
                  conversionLocked={queueLocked}
                  readOnly={!!libraryReadError}
                  onStorageError={reportLibraryError}
                  onReload={loadLibrary}
                  onReexport={handleQueueFromLibrary}
                />
              </Suspense>
            </TabsContent>
          </>
        )}
      </div>

      <Suspense fallback={null}>
        <SettingsPanel
          open={settingsOpen}
          onOpenChange={setSettingsOpen}
          prefs={{ ...prefs, cycleThemeTo: setThemeDirect }}
          updater={updater}
        />
      </Suspense>
    </Tabs>
  )
}
