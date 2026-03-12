// SPDX-License-Identifier: AGPL-3.0-or-later
import { Loader2 } from 'lucide-react';
import { cn } from '../../lib/utils';

/**
 * Standardized Spinner component
 *
 * Sizes:
 * - xs: 12px (h-3 w-3) - Badges, inline with small text
 * - sm: 16px (h-4 w-4) - Buttons, inline loading states
 * - md: 24px (h-6 w-6) - Cards, section loading
 * - lg: 32px (h-8 w-8) - Page section loading
 * - xl: 48px (h-12 w-12) - Full page loading
 *
 * Usage:
 *   <Spinner />                    // Default: sm size, inherits text color
 *   <Spinner size="lg" />          // Large spinner
 *   <Spinner className="mr-2" />   // With margin for button usage
 *   <Spinner className="text-primary" />  // Custom color
 */
export function Spinner({ size = 'sm', className }) {
  const sizeClasses = {
    xs: 'h-3 w-3',
    sm: 'h-4 w-4',
    md: 'h-6 w-6',
    lg: 'h-8 w-8',
    xl: 'h-12 w-12',
  };

  return (
    <Loader2
      className={cn(
        'animate-spin',
        sizeClasses[size] || sizeClasses.sm,
        className
      )}
    />
  );
}

/**
 * Centered spinner for page/section loading states
 *
 * Usage:
 *   <SpinnerPage />                     // Full page centered, lg size
 *   <SpinnerPage size="xl" />           // Larger spinner
 *   <SpinnerPage className="py-8" />    // Custom padding
 */
export function SpinnerPage({ size = 'lg', className }) {
  return (
    <div className={cn('flex items-center justify-center py-12', className)}>
      <Spinner size={size} className="text-muted-foreground" />
    </div>
  );
}

/**
 * Full page spinner with background
 * Used for initial page loads, route transitions
 */
export function SpinnerFullPage({ size = 'lg' }) {
  return (
    <div className="h-full flex items-center justify-center bg-background">
      <Spinner size={size} className="text-muted-foreground" />
    </div>
  );
}

export default Spinner;
