import { Ban, CircleCheck, CircleDashed, CircleX, Clock3, LoaderCircle } from 'lucide-react'
import { Badge } from '../ui/badge'

export default function StatusChip({ status }) {
  const map = {
    waiting: ['default', 'Waiting', Clock3],
    queued: ['default', 'Queued', CircleDashed],
    converting: ['active', 'Processing', LoaderCircle],
    done: ['done', 'Done', CircleCheck],
    error: ['error', 'Failed', CircleX],
    cancelled: ['warning', 'Cancelled', Ban],
  }
  const [variant, label, Icon] = map[status] || map.waiting

  return (
    <Badge variant={variant} className="gap-1">
      <Icon
        aria-hidden="true"
        className={status === 'converting' ? 'h-3 w-3 animate-spin motion-reduce:animate-none' : 'h-3 w-3'}
        strokeWidth={1.8}
      />
      {label}
    </Badge>
  )
}
