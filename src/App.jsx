import { useState, useEffect, useRef, lazy, Suspense } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { Sun, Moon, Monitor, Settings, AudioLines, Play, GitMerge, FolderOpen } from 'lucide-react'
import { basename } from './utils'

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
const PlayerTab  = lazy(() => import('./components/Player/PlayerTab'))
const MergeTab   = lazy(() => import('./components/Merge/MergeTab'))
const SettingsPanel = lazy(() => import('./components/SettingsPanel'))

const themeIcons = {
  light: Sun,
  dark: Moon,
  system: Monitor,
}

// Sidebar navigation: tab id → icon, label, and its number-key shortcut
const NAV = [
  { id: 'convert', label: 'Convert', Icon: AudioLines },
  { id: 'player',  label: 'Player',  Icon: Play },
  { id: 'merge',   label: 'Merge',   Icon: GitMerge },
  { id: 'library', label: 'Library', Icon: FolderOpen },
]

export default function App() {
  const [tab, setTab] = useState('convert')

  // Custom hooks
  const { themePref, themeLabel, cycleTheme, setThemeDirect } = useTheme()
  const [settingsOpen, setSettingsOpen] = useState(false)

  const prefs = usePreferencesContext()
  const { labels, outDir } = prefs

  // Auto-update via GitHub Releases (checks once on launch)
  const updater = useUpdater()

  // While the Player tab is mounted it registers its own drop handler here,
  // so native drops land in the playlist instead of the convert queue
  const dropOverrideRef = useRef(null)
  const fileDrop = useFileDrop(dropOverrideRef)
  const {
    files, setFiles, dragOver, caseName, setCaseName,
    onDragOver, onDragLeave, onDrop, browseFiles, browseOutDir,
    removeFile, clearAll,
  } = fileDrop

  const conversion = useConversion()
  const { jobs, converting, doneCount, failCount } = conversion

  // System capabilities (hardware-aware recommendations)
  const [capabilities, setCapabilities] = useState(null)
  useEffect(() => {
    invoke('system_capabilities_cmd').then(setCapabilities).catch(() => {})
  }, [])

  // Sidebar health card: sidecars + installed AI models
  const [health, setHealth] = useState(null)
  useEffect(() => {
    invoke('health_check').then(setHealth).catch(() => {})
  }, [])

  // Library state
  const [cases, setCases]     = useState([])
  const [libSearch, setLibSearch] = useState('')

  // Load library on startup (nav badge count) and when opening the tab
  useEffect(() => {
    invoke('library_get').then(setCases).catch(() => {})
  }, [])
  useEffect(() => {
    if (tab === 'library') {
      invoke('library_get').then(setCases).catch(() => {})
    }
  }, [tab])

  // Number keys 1–4 switch tabs (ignored while typing or with modifiers held)
  useEffect(() => {
    const onKey = (e) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return
      const el = e.target
      if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT' || el.isContentEditable)) return
      const idx = ['1', '2', '3', '4'].indexOf(e.key)
      if (idx >= 0) setTab(NAV[idx].id)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const handleStartConversion = () => {
    conversion.startConversion({
      files, outDir, ...prefs,
      caseName, setCases,
    })
  }

  const ThemeIcon = themeIcons[themeLabel] || Monitor
  const libCount = cases.filter(c => !c.archived).length
  const modelCount = health?.models?.length ?? null

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

        <TabsList aria-label="Main navigation" className="flex-col items-stretch gap-1 bg-transparent border-none rounded-none p-0 px-2 md:px-3 h-auto">
          {NAV.map(({ id, label, Icon }, i) => (
            <TabsTrigger key={id} value={id} className="w-full justify-start gap-2.5 px-2.5 md:px-3 py-2 rounded-lg">
              <Icon size={16} aria-hidden="true" className="shrink-0" />
              <span className="hidden md:inline">{label}</span>
              {id === 'library' && libCount > 0 && <Badge variant="gold" className="hidden md:inline-flex">{libCount}</Badge>}
              <kbd aria-hidden="true" className="hidden md:inline ml-auto font-mono text-[9.5px] px-1.5 py-px rounded border border-border/70 bg-secondary/60 text-[hsl(var(--sub))]">{i + 1}</kbd>
            </TabsTrigger>
          ))}
        </TabsList>

        <div className="flex-1" />

        {/* System health */}
        <div className="hidden md:block mx-3 mb-2 px-3 py-2.5 rounded-lg border border-border/70 bg-[hsl(var(--surface))]">
          <div className="flex items-center gap-2 py-0.5 text-[11px] font-medium text-[hsl(var(--text2))]">
            <span aria-hidden="true" className={`w-1.5 h-1.5 rounded-full ${health ? (health.ffmpeg ? 'bg-[hsl(var(--success))]' : 'bg-destructive') : 'bg-[hsl(var(--sub))]'}`} />
            {health ? (health.ffmpeg ? 'FFmpeg ready' : 'FFmpeg missing') : 'Checking engine…'}
          </div>
          {modelCount != null && (
            <div className="flex items-center gap-2 py-0.5 text-[11px] font-medium text-[hsl(var(--text2))]">
              <span aria-hidden="true" className={`w-1.5 h-1.5 rounded-full ${modelCount > 0 ? 'bg-[hsl(var(--success))]' : 'bg-[hsl(var(--warning))]'}`} />
              {modelCount} AI model{modelCount !== 1 ? 's' : ''} installed
            </div>
          )}
          <div className="py-0.5 text-[11px] text-[hsl(var(--sub))]">
            {updater.status === 'available' ? 'Update available' : 'Up to date'}
          </div>
        </div>

        <div className="flex md:justify-start justify-center items-center gap-1 px-2 md:px-3 pb-3">
          <Button variant="ghost" size="icon" title="Settings" aria-label="Settings" onClick={() => setSettingsOpen(true)}>
            <Settings size={16} aria-hidden="true" />
          </Button>
          <Button variant="ghost" size="icon" title={`Theme: ${themePref}`} aria-label={`Switch theme (current: ${themePref})`} onClick={cycleTheme}>
            <ThemeIcon size={16} aria-hidden="true" />
          </Button>
        </div>
      </aside>

      {/* ── Main column ── */}
      <div className="flex-1 flex flex-col min-w-0 overflow-hidden">
        <UpdateBanner updater={updater} />

        <TabsContent value="convert" forceMount={tab === 'convert' ? true : undefined}>
          {tab === 'convert' && (
            <ConvertTab
              capabilities={capabilities}
              files={files} dragOver={dragOver}
              caseName={caseName} setCaseName={setCaseName}
              onDragOver={onDragOver} onDragLeave={onDragLeave} onDrop={onDrop}
              browseFiles={browseFiles} browseOutDir={browseOutDir}
              removeFile={removeFile} clearAll={clearAll}
              jobs={jobs} converting={converting}
              startConversion={handleStartConversion}
              doneCount={doneCount} failCount={failCount}
            />
          )}
        </TabsContent>

        <TabsContent value="player">
          <Suspense fallback={<div className="flex items-center justify-center py-12"><Spinner className="h-5 w-5" /></div>}>
            <PlayerTab dropHandlerRef={dropOverrideRef} />
          </Suspense>
        </TabsContent>

        <TabsContent value="merge">
          <Suspense fallback={<div className="flex items-center justify-center py-12"><Spinner className="h-5 w-5" /></div>}>
            <MergeTab />
          </Suspense>
        </TabsContent>

        <TabsContent value="library">
          <Suspense fallback={<div className="flex items-center justify-center py-12"><Spinner className="h-5 w-5" /></div>}>
            <LibraryTab
              cases={cases} setCases={setCases}
              search={libSearch} setSearch={setLibSearch}
              labels={labels}
              onReexport={(srcPath, srcCaseName) => {
                setFiles([{path:srcPath, name:basename(srcPath), fmt:null}])
                setCaseName(srcCaseName || '')
                setTab('convert')
              }}
            />
          </Suspense>
        </TabsContent>
      </div>

      <Suspense fallback={null}>
        <SettingsPanel
          open={settingsOpen}
          onOpenChange={setSettingsOpen}
          prefs={{ ...prefs, themePref, cycleThemeTo: setThemeDirect }}
          updater={updater}
        />
      </Suspense>
    </Tabs>
  )
}
