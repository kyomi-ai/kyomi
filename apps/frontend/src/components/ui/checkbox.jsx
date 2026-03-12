// SPDX-License-Identifier: AGPL-3.0-or-later
import * as React from "react"
import { cn } from "@/lib/utils"
import { Check, Minus } from "lucide-react"

const Checkbox = React.forwardRef(({ className, checked, indeterminate, onCheckedChange, disabled, ...props }, ref) => {
  const handleClick = () => {
    if (!disabled && onCheckedChange) {
      onCheckedChange(!checked);
    }
  };

  const isChecked = checked || false;
  const isIndeterminate = indeterminate && !isChecked;

  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={isIndeterminate ? "mixed" : isChecked}
      data-state={isIndeterminate ? "indeterminate" : isChecked ? "checked" : "unchecked"}
      disabled={disabled}
      onClick={handleClick}
      className={cn(
        "peer inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm border border-input shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-50",
        (isChecked || isIndeterminate) ? "bg-primary border-primary text-primary-foreground" : "bg-background",
        className
      )}
      ref={ref}
      {...props}
    >
      {isChecked && <Check className="h-3 w-3" strokeWidth={3} />}
      {isIndeterminate && <Minus className="h-3 w-3" strokeWidth={3} />}
    </button>
  );
});

Checkbox.displayName = "Checkbox"

export { Checkbox }
