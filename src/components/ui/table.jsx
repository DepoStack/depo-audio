import * as React from 'react'
import { cn } from '../../lib/utils'

const Table = React.forwardRef(({ className, ...props }, ref) => (
  <div className="relative w-full overflow-auto">
    <table ref={ref} className={cn('w-full caption-bottom text-xs', className)} {...props} />
  </div>
))
Table.displayName = 'Table'

const TableHeader = React.forwardRef(({ className, ...props }, ref) => (
  <thead ref={ref} className={cn('[&_tr]:border-b', className)} {...props} />
))
TableHeader.displayName = 'TableHeader'

const TableBody = React.forwardRef(({ className, ...props }, ref) => (
  <tbody ref={ref} className={cn('[&_tr:last-child]:border-0', className)} {...props} />
))
TableBody.displayName = 'TableBody'

const TableRow = React.forwardRef(({ className, ...props }, ref) => (
  <tr ref={ref} className={cn('border-b border-border/60 transition-colors hover:bg-secondary/50', className)} {...props} />
))
TableRow.displayName = 'TableRow'

const TableHead = React.forwardRef(({ className, ...props }, ref) => (
  <th ref={ref} className={cn('h-8 px-4 text-left align-middle font-mono text-[9.5px] font-medium tracking-wider uppercase text-[hsl(var(--sub))]', className)} {...props} />
))
TableHead.displayName = 'TableHead'

const TableCell = React.forwardRef(({ className, ...props }, ref) => (
  <td ref={ref} className={cn('px-4 py-2 align-middle text-[11px]', className)} {...props} />
))
TableCell.displayName = 'TableCell'

export { Table, TableHeader, TableBody, TableRow, TableHead, TableCell }
