// SPDX-License-Identifier: AGPL-3.0-or-later
import * as React from "react"
import { cva } from "class-variance-authority"
import { cn } from "@/lib/utils"

const statusBarVariants = cva(
  "px-6 py-3.5 border-t flex items-center justify-between gap-4",
  {
    variants: {
      variant: {
        default: "bg-muted border-border",
        warning: "bg-warning border-warning-border",
        error: "bg-error border-error-border",
        success: "bg-success border-success-border",
        info: "bg-info border-info-border",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  }
)

function StatusBar({ className, variant, children, action, ...props }) {
  return (
    <div
      className={cn(statusBarVariants({ variant }), className)}
      {...props}
    >
      <div className="flex-1 flex items-center gap-3">
        {children}
      </div>
      {action && (
        <div className="flex-shrink-0">
          {action}
        </div>
      )}
    </div>
  )
}

const StatusBarText = React.forwardRef(({ className, variant = "default", ...props }, ref) => {
  const textColorMap = {
    default: "text-muted-foreground",
    warning: "text-warning-foreground",
    error: "text-error-foreground",
    success: "text-success-foreground",
    info: "text-info-foreground",
  }

  return (
    <span
      ref={ref}
      className={cn("text-sm", textColorMap[variant], className)}
      {...props}
    />
  )
})
StatusBarText.displayName = "StatusBarText"

export { StatusBar, StatusBarText, statusBarVariants }
