import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { MODES, FORMATS_OUT, CH_COLORS } from '../../constants'
import Toggle from '../common/Toggle'
import Spinner from '../common/Spinner'
import { ModeIcon, WaveformIcon } from '../common/Icons'
import FormatTable from './FormatTable'
import FileRow from './FileRow'

export default function ConvertTab({
  // Preferences
  mode, setMode, formatOut, setFormatOut, labels, setLabels,
  chanVols, setChanVols, outDir, setOutDir, rate, setRate,
  normalize, setNormalize, trim, setTrim, fade, setFade,
  fadeDur, setFadeDur, hpf, setHpf,
  denoise, setDenoise, denoiseQuality, setDenoiseQuality,
  autoLevel, setAutoLevel, declip, setDeclip, enhance, setEnhance,
  capabilities,
  // Files
  files, dragOver, caseName, setCaseName,
  onDragOver, onDragLeave, onDrop, browseFiles, browseOutDir,
  removeFile, clearAll,
  // Conversion
  jobs, converting, startConversion, doneCount, failCount,
}) {
  const anyProc = normalize || trim || fade || hpf
  const anyAi = denoise || autoLevel || declip || enhance
  const [analysis, setAnalysis] = useState(null)
  const [scanning, setScanning] = useState(false)

  const handleScan = async () => {
    if (!files.length || scanning) return
    setScanning(true)
    try {
      const result = await invoke('analyze_audio_cmd', { path: files[0].path })
      setAnalysis(result)
      // Auto-enable recommended features
      if (result.needsDenoise) setDenoise(true)
      if (result.needsLeveling) setAutoLevel(true)
      if (result.hasClipping) setDeclip(true)
      if (result.isNarrowband) setEnhance(true)
    } catch (e) {
      console.error('Scan failed:', e)
    }
    setScanning(false)
  }

  return (
    <>
      <div className="main-scroll">
        <div className="content">

          {/* ── OUTPUT MODE ──────────────────────────────────────────────── */}
          <section className="panel">
            <div className="panel-head"><span className="panel-label">OUTPUT MODE</span></div>
            <div className="mode-grid">
              {MODES.map(m => (
                <button key={m.id} className={`mode-card${mode===m.id?' mode-card--active':''}`} onClick={() => setMode(m.id)}>
                  <ModeIcon id={m.id} active={mode===m.id} />
                  <div className="mode-card-body">
                    <span className="mode-name">{m.label}</span>
                    <span className="mode-desc">{m.desc}</span>
                  </div>
                  {mode===m.id && <span className="mode-check">✓</span>}
                </button>
              ))}
            </div>
          </section>

          {/* ── CHANNEL LABELS ──────────────────────────────────────────── */}
          <section className="panel panel--tight">
            <div className="panel-head">
              <span className="panel-label">CHANNEL LABELS</span>
              <span className="panel-hint">
                {mode === 'split'  && 'Used as output filenames when splitting channels'}
                {mode === 'stereo' && 'Labels shown in mix sliders below'}
                {mode === 'keep'   && 'Labels saved to library for reference'}
              </span>
            </div>
            <div className="ch-grid">
              {labels.map((l,i) => (
                <div key={i} className="ch-item">
                  <span className="ch-dot" style={{background:CH_COLORS[i]}} />
                  <span className="ch-num">CH {i+1}</span>
                  <input className="ch-input" value={l} maxLength={24} placeholder={`Channel ${i+1}`}
                    onChange={e => setLabels(p => p.map((v,j) => j===i ? e.target.value:v))} />
                </div>
              ))}
            </div>
          </section>

          {/* ── CHANNEL MIX ─────────────────────────────────────────────── */}
          <section className={`panel panel--tight${mode !== 'stereo' || autoLevel ? ' panel--muted' : ''}`}>
            <div className="panel-head">
              <span className="panel-label">CHANNEL MIX</span>
              {autoLevel
                ? <span className="panel-hint panel-hint--inactive">Managed by Balance Speaker Volume above</span>
                : mode !== 'stereo'
                  ? <span className="panel-hint panel-hint--inactive">Active in Mix to Stereo mode</span>
                  : <span className="panel-hint">Per-channel volume — 1.0 = unity, 0.0 = mute, 2.0 = boost</span>
              }
            </div>
            <div className="chan-vols-grid">
              {chanVols.map((v,i) => (
                <div key={i} className={`chan-vol-item${mode !== 'stereo' || autoLevel ? ' chan-vol-item--dim' : ''}`}>
                  <span className="chan-vol-dot" style={{background:CH_COLORS[i]}} />
                  <span className="chan-vol-name">{labels[i]||`CH ${i+1}`}</span>
                  <input type="range" min="0" max="2" step="0.05" value={autoLevel ? 1.0 : v} className="chan-vol-slider"
                    style={{'--fill':`${((autoLevel ? 1.0 : v)/2)*100}%`}}
                    disabled={mode !== 'stereo' || autoLevel}
                    onChange={e => setChanVols(p => p.map((x,j) => j===i ? parseFloat(e.target.value):x))} />
                  <span className="chan-vol-val">{autoLevel ? 'auto' : v===0?'mute':v.toFixed(2)}</span>
                </div>
              ))}
            </div>
          </section>

          {/* ── SMART AUDIO CLEANUP ──────────────────────────────────────── */}
          <section className="panel panel--tight panel--ai">
            <div className="panel-head">
              <span className="panel-label">SMART AUDIO CLEANUP</span>
              <button className="btn btn--sm btn--scan" onClick={handleScan}
                disabled={!files.length || scanning || converting}>
                {scanning ? 'Scanning…' : 'Scan'}
              </button>
            </div>

            {!analysis && !scanning && (
              <p className="ai-hint">Drop files and click <strong>Scan</strong> to automatically detect audio issues and apply the right fixes.</p>
            )}

            {scanning && (
              <p className="ai-hint"><Spinner /> Listening to your audio…</p>
            )}

            {analysis && (
              <div className="ai-results">
                {analysis.qualityScore && (
                  <span className="ai-result-chip" title={`Signal: ${analysis.qualityScore.sig?.toFixed(1)} · Background: ${analysis.qualityScore.bak?.toFixed(1)}`}>
                    Quality: {analysis.qualityScore.ovr?.toFixed(1)}/5
                  </span>
                )}
                {analysis.speakerCount != null && (
                  <span className="ai-result-chip">
                    {analysis.speakerCount} speaker{analysis.speakerCount !== 1 ? 's' : ''} detected
                  </span>
                )}
                {analysis.turns?.length > 0 && (
                  <span className="ai-result-chip">
                    {analysis.turns.length} turn{analysis.turns.length !== 1 ? 's' : ''} found
                  </span>
                )}
              </div>
            )}

            <div className="proc-grid">
              <label className="proc-item proc-item--ai">
                <div className="proc-item-info">
                  <span className="proc-name">Remove Background Noise</span>
                  <span className="proc-desc">
                    Cleans up HVAC hum, paper rustling, and room noise — keeps speech clear
                    {analysis?.needsDenoise && <span className="ai-detected"> · Noise detected</span>}
                    {denoise && (
                      <span className="denoise-quality" onClick={e => e.preventDefault()}>
                        &nbsp;—&nbsp;
                        <select className="denoise-select" value={denoiseQuality}
                          onChange={e => { e.stopPropagation(); setDenoiseQuality(e.target.value) }}>
                          <option value="fast">Fast</option>
                          <option value="best">Best quality</option>
                        </select>
                      </span>
                    )}
                  </span>
                </div>
                <Toggle checked={denoise} onChange={setDenoise} />
              </label>

              <label className="proc-item proc-item--ai">
                <div className="proc-item-info">
                  <span className="proc-name">Balance Speaker Volume</span>
                  <span className="proc-desc">
                    Evens out volume so quiet speakers are easier to hear
                    {analysis?.needsLeveling && <span className="ai-detected"> · {analysis.recommendations?.find(r => r.includes('spread'))?.match(/[\d.]+/)?.[0] || ''}dB imbalance found</span>}
                  </span>
                </div>
                <Toggle checked={autoLevel} onChange={v => { setAutoLevel(v); }} />
              </label>

              <label className="proc-item proc-item--ai">
                <div className="proc-item-info">
                  <span className="proc-name">Fix Clipped Audio</span>
                  <span className="proc-desc">
                    Repairs distorted peaks from recordings that were too loud
                    {analysis?.hasClipping && <span className="ai-detected"> · Clipping found</span>}
                  </span>
                </div>
                <Toggle checked={declip} onChange={setDeclip} />
              </label>

              <label className="proc-item proc-item--ai">
                <div className="proc-item-info">
                  <span className="proc-name">Enhance Clarity</span>
                  <span className="proc-desc">
                    Improves phone recordings and narrow-band audio for easier listening
                    {analysis?.isNarrowband && <span className="ai-detected"> · {analysis.sampleRate?.toLocaleString() || ''}Hz audio detected</span>}
                  </span>
                </div>
                <Toggle checked={enhance} onChange={setEnhance} />
              </label>
            </div>

            {/* Processing pipeline preview */}
            {anyAi && (
              <div className="proc-chain">
                <span className="proc-chain-label">CLEANUP</span>
                <div className="proc-chain-steps">
                  {denoise   && <><span className="proc-chip proc-chip--ai">Denoise</span><span className="proc-chain-arrow">→</span></>}
                  {enhance   && <><span className="proc-chip proc-chip--ai">Enhance</span><span className="proc-chain-arrow">→</span></>}
                  {declip    && <><span className="proc-chip proc-chip--ai">De-clip</span><span className="proc-chain-arrow">→</span></>}
                  {autoLevel && <span className="proc-chip proc-chip--ai">Auto-Level</span>}
                </div>
              </div>
            )}

            <p className="ai-privacy">
              All processing runs on your machine — nothing is uploaded or sent anywhere.
              {capabilities && (
                <span className="ai-hw-hint" title={`${capabilities.cpuCores} cores · ${Math.round(capabilities.ramMb/1024)}GB RAM${capabilities.appleSilicon ? ' · Apple Silicon' : ''}`}>
                  {' '}· {capabilities.tier === 'high' ? 'High performance' : capabilities.tier === 'mid' ? 'Standard performance' : 'Lightweight mode'}
                </span>
              )}
            </p>
          </section>

          {/* ── PROCESSING ───────────────────────────────────────────────── */}
          <section className="panel panel--tight">
            <div className="panel-head"><span className="panel-label">PROCESSING</span></div>
            <div className="proc-grid">
              <label className="proc-item">
                <div className="proc-item-info">
                  <span className="proc-name">High-Pass Filter</span>
                  <span className="proc-desc">80 Hz cutoff — removes HVAC hum, mic handling noise, low rumble</span>
                </div>
                <Toggle checked={hpf} onChange={setHpf} />
              </label>
              <label className="proc-item">
                <div className="proc-item-info">
                  <span className="proc-name">Normalize</span>
                  <span className="proc-desc">Loudnorm — evens out quiet recordings, targets –16 LUFS / –1.5 TP</span>
                </div>
                <Toggle checked={normalize} onChange={setNormalize} />
              </label>
              <label className="proc-item">
                <div className="proc-item-info">
                  <span className="proc-name">Trim Silence</span>
                  <span className="proc-desc">Remove leading and trailing silence below –50 dB / 0.3s minimum</span>
                </div>
                <Toggle checked={trim} onChange={setTrim} />
              </label>
              <label className="proc-item">
                <div className="proc-item-info">
                  <span className="proc-name">Fade In / Out</span>
                  <span className="proc-desc">
                    Smooth start and end
                    {fade && (
                      <span className="fade-dur-wrap" onClick={e => e.preventDefault()}>
                        &nbsp;—&nbsp;
                        <input type="number" className="fade-dur-input" min="0.1" max="5" step="0.1" value={fadeDur}
                          onChange={e => setFadeDur(Math.max(0.1, parseFloat(e.target.value)||0.5))} />
                        <span className="fade-dur-unit">s</span>
                      </span>
                    )}
                  </span>
                </div>
                <Toggle checked={fade} onChange={setFade} />
              </label>
            </div>

            {/* Live processing chain preview */}
            <div className="proc-chain">
              <span className="proc-chain-label">CHAIN</span>
              {(anyProc || anyAi) ? (
                <div className="proc-chain-steps">
                  {denoise   && <><span className="proc-chip proc-chip--ai">Denoise</span><span className="proc-chain-arrow">→</span></>}
                  {enhance   && <><span className="proc-chip proc-chip--ai">Enhance</span><span className="proc-chain-arrow">→</span></>}
                  {declip    && <><span className="proc-chip proc-chip--ai">De-clip</span><span className="proc-chain-arrow">→</span></>}
                  {hpf       && <><span className="proc-chip proc-chip--on">HPF 80Hz</span><span className="proc-chain-arrow">→</span></>}
                  {autoLevel && <><span className="proc-chip proc-chip--ai">Auto-Level</span><span className="proc-chain-arrow">→</span></>}
                  {normalize && <><span className="proc-chip proc-chip--on">Loudnorm</span><span className="proc-chain-arrow">→</span></>}
                  {trim      && <><span className="proc-chip proc-chip--on">Trim silence</span><span className="proc-chain-arrow">→</span></>}
                  {fade      && <span className="proc-chip proc-chip--on">Fade {fadeDur}s</span>}
                </div>
              ) : (
                <span className="proc-chain-empty">No processing — direct transcode only</span>
              )}
            </div>
          </section>

          {/* ── OPTIONS ──────────────────────────────────────────────────── */}
          <div className="opts-row">
            <div className="opt-block opt-block--grow">
              <label className="opt-label">CASE NAME</label>
              <input className="opt-input" value={caseName} placeholder="Auto-detected from filename — override here"
                onChange={e => setCaseName(e.target.value)} />
            </div>
            <div className="opt-block opt-block--grow">
              <label className="opt-label">OUTPUT FOLDER</label>
              <div className="opt-inline">
                <input className="opt-input" value={outDir} placeholder="Default: same folder as source"
                  onChange={e => setOutDir(e.target.value)} />
                <button className="btn btn--sm" onClick={() => browseOutDir(setOutDir)}>Browse</button>
              </div>
            </div>
            <div className="opt-block">
              <label className="opt-label">SAMPLE RATE</label>
              <select className="opt-select" value={formatOut === 'opus' ? '48000' : rate}
                disabled={formatOut === 'opus'}
                title={formatOut === 'opus' ? 'Opus is always 48 kHz' : ''}
                onChange={e => setRate(e.target.value)}>
                <option value="22050">22,050 Hz</option>
                <option value="44100">44,100 Hz</option>
                <option value="48000">48,000 Hz</option>
              </select>
            </div>
            <div className="opt-block">
              <label className="opt-label">OUTPUT FORMAT</label>
              <div className="format-tabs">
                {FORMATS_OUT.map(f => (
                  <button key={f.id} title={f.desc}
                    className={`fmt-tab${formatOut===f.id?' fmt-tab--active':''}`}
                    onClick={() => setFormatOut(f.id)}>{f.label}</button>
                ))}
              </div>
            </div>
          </div>

          <FormatTable />

          {/* Drop zone */}
          <div className={`dropzone${dragOver?' dropzone--over':''}`}
            onDragOver={onDragOver} onDragLeave={onDragLeave} onDrop={onDrop}
            onClick={browseFiles}>
            <WaveformIcon />
            <p className="drop-title">Drop audio files here</p>
            <p className="drop-sub">or <span className="drop-link">click to browse</span> — WAV · MP3 · FLAC · Opus · M4A · OGG · SGMCA · TRM · BWF and more</p>
          </div>

          {files.length > 0 && (
            <div className="filelist-wrap">
              <div className="filelist-head">
                <span className="filelist-count">{files.length} file{files.length!==1?'s':''} queued</span>
                {!converting && <button className="ghost-btn" onClick={() => clearAll(converting)}>Clear all</button>}
              </div>
              <div className="filelist">
                {files.map(f => <FileRow key={f.path} file={f} job={jobs[f.path]} onRemove={() => removeFile(f.path, converting)} converting={converting} />)}
              </div>
            </div>
          )}

        </div>
      </div>

      <footer className="bottombar">
        <div className="bottombar-status">
          {converting && <span className="status-pill status-pill--active"><span className="status-dot"/>{doneCount > 0 ? `${doneCount} / ${files.length} done` : 'Converting…'}</span>}
          {!converting && doneCount > 0 && <span className="status-pill status-pill--done">✓ {doneCount} file{doneCount!==1?'s':''} converted{failCount>0?`, ${failCount} failed`:''}</span>}
        </div>
        <button className={`btn btn--primary${converting||!files.length?' btn--disabled':''}`}
          onClick={startConversion} disabled={converting||!files.length}>
          {converting ? <><Spinner />Converting…</> : <>▶ Convert{files.length > 1 ? ` ${files.length} Files` : ''}</>}
        </button>
      </footer>
    </>
  )
}
