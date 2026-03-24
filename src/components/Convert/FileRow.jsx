import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { CH_COLORS } from '../../constants'
import StatusChip from '../common/StatusChip'
import MiniPlayer from './MiniPlayer'

export default function FileRow({ file, job, onRemove, converting }) {
  const [expanded, setExpanded] = useState(false)
  const status = job?.status || 'waiting'
  const isExp = file.fmt?.status === 'experimental'
  const isRej = file.fmt?.status === 'unsupported'

  return (
    <div className={`fr fr--${status}${isRej?' fr--rejected':''}`}>
      <div className="fr-main">
        <div className={`fi ${status==='done'?'fi--done':status==='error'?'fi--error':status==='converting'?'fi--active':isRej?'fi--bad':''}`}>
          <svg width="18" height="22" viewBox="0 0 18 22" fill="none">
            <path d="M2 2h9l5 5v13a1 1 0 01-1 1H2a1 1 0 01-1-1V3a1 1 0 011-1z" stroke="currentColor" strokeWidth="1.4"/>
            <path d="M11 2v5h5" stroke="currentColor" strokeWidth="1.4" strokeLinejoin="round"/>
            <line x1="4" y1="12" x2="14" y2="12" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" opacity=".55"/>
            <line x1="4" y1="15.5" x2="11" y2="15.5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" opacity=".35"/>
          </svg>
        </div>
        <div className="fr-info">
          <div className="fr-top">
            <span className="fr-name" title={file.path}>{file.name}</span>
            {file.fmt && <span className={`fr-tag${isExp?' fr-tag--exp':isRej?' fr-tag--bad':''}`}>{file.fmt.name.split('·')[0].trim()}</span>}
          </div>
          <span className="fr-path">{file.path}</span>
        </div>
        <div className="fr-right">
          <StatusChip status={status} />
          {!converting && <button className="fr-remove" onClick={onRemove}>
            <svg width="9" height="9" viewBox="0 0 9 9"><path d="M1 1l7 7M8 1L1 8" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/></svg>
          </button>}
        </div>
      </div>
      {status === 'converting' && (
        <div className="fr-progress">
          {job?.phase && <span className="fr-phase">{job.phase === 'analyzing' ? 'Listening to audio…' : job.phase === 'processing' ? 'Cleaning up audio…' : 'Converting…'}</span>}
          <div className="progress-track"><div className="progress-fill"/></div>
        </div>
      )}
      {status === 'done' && job.outputs?.length > 0 && (
        <div className="fr-outputs">
          {job.outputs.map((out, i) => <MiniPlayer key={i} out={out} color={CH_COLORS[i%4]} multi={job.outputs.length > 1} />)}
          {job.outputs.length > 1 && (
            <button className="show-folder-btn"
              onClick={() => invoke('show_in_folder', { path: job.outputs[0].path }).catch(() => {})}>
              <svg width="11" height="11" viewBox="0 0 11 11" fill="none">
                <path d="M1 2.5h3.5l1 1H10v6H1V2.5z" stroke="currentColor" strokeWidth="1.2" fill="none" strokeLinejoin="round"/>
              </svg>
              Show in Explorer / Finder
            </button>
          )}
        </div>
      )}
      {status === 'error' && (
        <div className="fr-error">
          <button className="err-toggle" onClick={() => setExpanded(e => !e)}>{expanded?'▲ hide':'▼ details'}</button>
          {expanded && <pre className="err-text">{job.error}</pre>}
        </div>
      )}
    </div>
  )
}
