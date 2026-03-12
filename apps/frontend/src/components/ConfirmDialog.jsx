// SPDX-License-Identifier: AGPL-3.0-or-later
import { createPortal } from 'react-dom';
import { Button } from './ui/button';

/**
 * ConfirmDialog Component
 *
 * A modern replacement for browser native confirm() dialogs.
 * Provides a consistent, customizable confirmation experience that:
 * - Matches the application design system
 * - Is non-blocking (unlike window.confirm)
 * - Supports keyboard navigation (Enter = confirm, Escape = cancel)
 * - Can be styled with variants (default, destructive)
 *
 * @param {boolean} isOpen - Whether to show the dialog
 * @param {function} onConfirm - Callback when user confirms
 * @param {function} onCancel - Callback when user cancels (backdrop click, Escape key, or Cancel button)
 * @param {string} title - Dialog title (e.g., "Delete Chat?")
 * @param {string} message - Dialog message (e.g., "This action cannot be undone.")
 * @param {string} confirmText - Text for confirm button (default: "Confirm")
 * @param {string} cancelText - Text for cancel button (default: "Cancel")
 * @param {string} variant - Button variant: 'default' or 'destructive' (default: 'default')
 *
 * @example
 * // Destructive action (delete, cancel subscription)
 * <ConfirmDialog
 *   isOpen={showDeleteConfirm}
 *   onConfirm={handleDelete}
 *   onCancel={() => setShowDeleteConfirm(false)}
 *   title="Delete Chat?"
 *   message="Are you sure you want to delete this chat? This action cannot be undone."
 *   confirmText="Delete"
 *   variant="destructive"
 * />
 *
 * @example
 * // Non-destructive confirmation (save changes, proceed)
 * <ConfirmDialog
 *   isOpen={showSaveConfirm}
 *   onConfirm={handleSave}
 *   onCancel={() => setShowSaveConfirm(false)}
 *   title="Save Changes?"
 *   message="Do you want to save your changes before leaving?"
 *   confirmText="Save"
 *   variant="default"
 * />
 */
const ConfirmDialog = ({
  isOpen,
  onConfirm,
  onCancel,
  title,
  message,
  confirmText = 'Confirm',
  cancelText = 'Cancel',
  variant = 'default'
}) => {
  if (!isOpen) return null;

  // Handle keyboard events
  const handleKeyDown = (e) => {
    if (e.key === 'Escape') {
      onCancel();
    } else if (e.key === 'Enter') {
      onConfirm();
    }
  };

  const dialogContent = (
    <div
      className="modal-overlay"
      onClick={onCancel}
      onKeyDown={handleKeyDown}
    >
      <div
        className="modal-content max-w-md w-full mx-4"
        onClick={(e) => e.stopPropagation()}
        role="alertdialog"
        aria-labelledby="confirm-dialog-title"
        aria-describedby="confirm-dialog-message"
      >
        {/* Header */}
        <div className="px-6 py-4 border-b border-border">
          <h2
            id="confirm-dialog-title"
            className="text-xl font-semibold text-foreground"
          >
            {title}
          </h2>
        </div>

        {/* Message */}
        <div className="p-6">
          <p
            id="confirm-dialog-message"
            className="text-muted-foreground"
          >
            {message}
          </p>
        </div>

        {/* Footer with buttons */}
        <div className="px-6 py-4 border-t border-border flex justify-end gap-2">
          <Button
            variant="outline"
            onClick={onCancel}
            autoFocus={variant === 'destructive'} // Focus cancel for destructive actions
          >
            {cancelText}
          </Button>
          <Button
            variant={variant}
            onClick={onConfirm}
            autoFocus={variant === 'default'} // Focus confirm for non-destructive actions
          >
            {confirmText}
          </Button>
        </div>
      </div>
    </div>
  );

  return createPortal(dialogContent, document.body);
};

export default ConfirmDialog;
