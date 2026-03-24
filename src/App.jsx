import { useState, useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import * as Tabs from '@radix-ui/react-tabs'
import { basename } from './utils'

import useTheme from './hooks/useTheme'
import usePreferences from './hooks/usePreferences'
import useFileDrop from './hooks/useFileDrop'
import useConversion from './hooks/useConversion'

import { LogoSvg } from './components/common/Icons'
import ConvertTab from './components/Convert/ConvertTab'
import LibraryTab from './components/Library/LibraryTab'
import PlayerTab from './components/Player/PlayerTab'

export default function App() {
  const [tab, setTab] = useState('convert')

  // Custom hooks
  const { themePref, themeLabel, cycleTheme } = useTheme()

  const prefs = usePreferences()
  const {
    mode, setMode, formatOut, setFormatOut, labels, setLabels,
    chanVols, setChanVols, outDir, setOutDir, rate, setRate,
    normalize, setNormalize, trim, setTrim, fade, setFade,
    fadeDur, setFadeDur, hpf, setHpf,
    denoise, setDenoise, denoiseQuality, setDenoiseQuality,
    autoLevel, setAutoLevel, declip, setDeclip, enhance, setEnhance,
    dereverb, setDereverb,
  } = prefs

  const fileDrop = useFileDrop()
  const {
    files, setFiles, dragOver, caseName, setCaseName,
    onDragOver, onDragLeave, onDrop, browseFiles, browseOutDir,
    removeFile, clearAll,
  } = fileDrop

  const conversion = useConversion()
  const { jobs, setJobs, converting, doneCount, failCount } = conversion

  // System capabilities (hardware-aware recommendations)
  const [capabilities, setCapabilities] = useState(null)
  useEffect(() => {
    invoke('system_capabilities_cmd').then(setCapabilities).catch(() => {})
  }, [])

  // Library state
  const [cases, setCases]     = useState([])
  const [libSearch, setLibSearch] = useState('')

  // Load library when switching to library tab
  useEffect(() => {
    if (tab === 'library') {
      invoke('library_get').then(setCases).catch(() => {})
    }
  }, [tab])

  const handleStartConversion = () => {
    conversion.startConversion({
      files, outDir, mode, formatOut, rate,
      labels, chanVols, normalize, trim, fade, fadeDur, hpf,
      denoise, denoiseQuality, autoLevel, declip, enhance, dereverb,
      caseName, setCases,
    })
  }

  return (
    <Tabs.Root value={tab} onValueChange={setTab} className="app">
      {/* ── Topbar */}
      <header className="topbar">
        <div className="topbar-brand">
          <LogoSvg />
          <div className="topbar-text">
            <span className="topbar-title">DepoAudio</span>
            <span className="topbar-tagline">Audio Converter &amp; Enhancer</span>
          </div>
        </div>
        <Tabs.List className="topbar-tabs" aria-label="Main navigation">
          <Tabs.Trigger value="convert" className="tab-btn">Convert</Tabs.Trigger>
          <Tabs.Trigger value="player" className="tab-btn">Player</Tabs.Trigger>
          <Tabs.Trigger value="library" className="tab-btn">
            Library {cases.filter(c=>!c.archived).length > 0 && <span className="tab-badge">{cases.filter(c=>!c.archived).length}</span>}
          </Tabs.Trigger>
        </Tabs.List>
        <div className="topbar-right">
          <button className="theme-btn" title={`Theme: ${themePref}`} onClick={cycleTheme}>{themeLabel}</button>
        </div>
      </header>

      <Tabs.Content value="convert" className="tab-content" forceMount={tab === 'convert' ? true : undefined}>
        {tab === 'convert' && (
          <ConvertTab
            mode={mode} setMode={setMode}
            formatOut={formatOut} setFormatOut={setFormatOut}
            labels={labels} setLabels={setLabels}
            chanVols={chanVols} setChanVols={setChanVols}
            outDir={outDir} setOutDir={setOutDir}
            rate={rate} setRate={setRate}
            normalize={normalize} setNormalize={setNormalize}
            trim={trim} setTrim={setTrim}
            fade={fade} setFade={setFade}
            fadeDur={fadeDur} setFadeDur={setFadeDur}
            hpf={hpf} setHpf={setHpf}
            denoise={denoise} setDenoise={setDenoise}
            denoiseQuality={denoiseQuality} setDenoiseQuality={setDenoiseQuality}
            autoLevel={autoLevel} setAutoLevel={setAutoLevel}
            declip={declip} setDeclip={setDeclip}
            enhance={enhance} setEnhance={setEnhance}
            dereverb={dereverb} setDereverb={setDereverb}
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
      </Tabs.Content>

      <Tabs.Content value="player" className="tab-content">
        <PlayerTab />
      </Tabs.Content>

      <Tabs.Content value="library" className="tab-content">
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
      </Tabs.Content>
    </Tabs.Root>
  )
}
