// SPDX-License-Identifier: AGPL-3.0-or-later
import { createPortal } from 'react-dom';
import { Button } from './ui/button';

/**
 * Base Modal Component
 *
 * Provides a consistent modal experience across the application with:
 * - Semi-transparent backdrop overlay
 * - White content background
 * - Proper font inheritance (system-ui)
 * - Portal rendering (escapes DOM hierarchy)
 * - Configurable size
 *
 * @param {boolean} show - Whether to show the modal
 * @param {function} onClose - Callback when backdrop is clicked
 * @param {string} title - Modal title
 * @param {node} children - Modal content
 * @param {string} size - Modal size: 'sm', 'md', 'lg', 'xl', 'full' (default: 'lg')
 * @param {node} footer - Optional footer content (buttons, etc.)
 */
const Modal = ({
  show,
  onClose,
  title,
  children,
  size = 'lg',
  footer
}) => {
  if (!show) return null;

  // Track if mousedown started on overlay (for proper backdrop click handling)
  let mouseDownOnOverlay = false;

  const handleOverlayMouseDown = (e) => {
    // Only set flag if click is directly on overlay, not on modal content
    if (e.target === e.currentTarget) {
      mouseDownOnOverlay = true;
    }
  };

  const handleOverlayClick = (e) => {
    // Only close if both mousedown and click happened on overlay
    // This prevents closing when user is selecting text and mouse goes outside modal
    if (e.target === e.currentTarget && mouseDownOnOverlay) {
      onClose();
    }
    mouseDownOnOverlay = false;
  };

  // Size class mapping
  const sizeClasses = {
    sm: 'max-w-sm',
    md: 'max-w-md',
    lg: 'max-w-4xl',
    xl: 'max-w-6xl',
    full: 'max-w-[95vw]'
  };

  const modalContent = (
    <div
      className="modal-overlay"
      onMouseDown={handleOverlayMouseDown}
      onClick={handleOverlayClick}
    >
      <div
        className={`modal-content ${sizeClasses[size]} w-full mx-2 sm:mx-4 max-h-[95vh] sm:max-h-[90vh] flex flex-col`}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        {title && (
          <div className="px-4 sm:px-6 py-3 sm:py-4 border-b border-border flex items-center justify-between flex-shrink-0">
            <h2 className="text-lg sm:text-xl font-semibold text-foreground">{title}</h2>
            <Button
              variant="ghost"
              size="icon"
              onClick={onClose}
              className="text-muted-foreground hover:text-foreground"
              aria-label="Close"
            >
              <svg className="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </Button>
          </div>
        )}

        {/* Content - scrollable */}
        <div className="p-4 sm:p-6 overflow-y-auto flex-1">
          {children}
        </div>

        {/* Footer */}
        {footer && (
          <div className="px-4 sm:px-6 py-3 sm:py-4 border-t border-border flex justify-end gap-2 flex-shrink-0">
            {footer}
          </div>
        )}
      </div>
    </div>
  );

  return createPortal(modalContent, document.body);
};

export default Modal;
