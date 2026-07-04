import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { CH_COLORS } from '../../constants'
import { FileAudio, X, FolderOpen, ChevronDown, ChevronUp } from 'lucide-react'
import { Badge } from '../ui/badge'
import { cn } from '../../lib/utils'
import StatusChip from '../common/StatusChip'
import MiniPlayer from './MiniPlayer'

export default function FileRow({ file, job, onRemove, converting }) {
  const [expanded, setExpanded] = useState(false)
  const status = job?.status || 'waiting'
  const isExp = file.fmt?.status === 'experimental'
  const isRej = file.fmt?.status === 'unsupported'

  return (
    <div className={cn(
      'bg-card border border-border rounded-lg overflow-hidden transition-colors',
      status === 'converting' && 'border-primary',
      status === 'done' && 'border-success/40',
      status === 'error' && 'border-destructive/40',
      isRej && 'opacity-60'
    )}>
      <div className="flex items-center gap-3 px-3 py-2.5">
        <div className={cn(
          'text-[hsl(var(--sub))] shrink-0',
          status === 'done' && 'text-success',
          status === 'error' && 'text-destructive',
          status === 'converting' && 'text-foreground',
          isRej && 'text-destructive/60'
        )}>
          <FileAudio size={18} />
        </div>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <span className="text-[13px] font-semibold text-foreground truncate" title={file.path}>{file.name}</span>
            {file.fmt && (
              <Badge variant={isRej ? 'error' : isExp ? 'warning' : 'tag'}>
                {file.fmt.name.split('·')[0].trim()}
              </Badge>
            )}
          </div>
          <span className="text-[10px] text-[hsl(var(--sub))] truncate block">{file.path}</span>
        </div>
        <div className="flex items-center gap-2 shrink-0">
          <StatusChip status={status} />
          {!converting && (
            <button className="w-5 h-5 rounded flex items-center justify-center text-[hsl(var(--sub))] hover:text-destructive hover:bg-destructive/10 transition-colors"
              aria-label="Remove file"
              onClick={onRemove}>
              <X size={9} />
            </button>
          )}
        </div>
      </div>
      {status === 'converting' && (
        <div className="px-3 pb-2.5">
          {job?.phase && (
            <span className="text-[10px] text-[hsl(var(--sub))] block mb-1">
              {job.phase === 'analyzing' ? 'Analyzing audio…' : job.phase === 'processing' ? 'Removing noise…' : `Encoding…`}{job.seconds > 0 ? ` ${Math.round(job.seconds)}s` : ''}
            </span>
          )}
          <div className="w-full h-1 bg-border rounded-full overflow-hidden">
            <div className="h-full bg-primary rounded-full animate-[loading_1.2s_ease-in-out_infinite]" />
          </div>
        </div>
      )}
      {status === 'done' && job.outputs?.length > 0 && (
        <div className="px-3 pb-2.5 flex flex-col gap-0.5">
          {job.outputs.map((out, i) => <MiniPlayer key={out.path} out={out} color={CH_COLORS[i%4]} multi={job.outputs.length > 1} />)}
          {job.outputs.length > 1 && (
            <button className="flex items-center gap-1 text-[10px] text-[hsl(var(--sub))] hover:text-foreground transition-colors mt-1 self-start"
              onClick={() => invoke('show_in_folder', { path: job.outputs[0].path }).catch(() => {})}>
              <FolderOpen size={11} />
              Show in Explorer / Finder
            </button>
          )}
        </div>
      )}
      {status === 'error' && (
        <div className="px-3 pb-2.5">
          <button className="text-[10px] text-[hsl(var(--sub))] hover:text-foreground transition-colors flex items-center gap-1"
            onClick={() => setExpanded(e => !e)}>
            {expanded ? <><ChevronUp size={10} /> hide</> : <><ChevronDown size={10} /> details</>}
          </button>
          {expanded && <pre className="mt-1 text-[10px] text-destructive font-mono whitespace-pre-wrap break-all bg-destructive/5 rounded p-2">{job.error}</pre>}
        </div>
      )}
    </div>
  )
}
