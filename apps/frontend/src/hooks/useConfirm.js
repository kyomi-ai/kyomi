// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useCallback } from 'react';

/**
 * useConfirm Hook
 *
 * Simplifies usage of ConfirmDialog by managing state and providing
 * a promise-based API. This makes it easy to replace window.confirm()
 * with a design-system-compliant dialog.
 *
 * @returns {Object} Hook interface
 * @returns {boolean} isOpen - Whether dialog is currently open
 * @returns {Object} dialogProps - Props to spread onto ConfirmDialog component
 * @returns {function} confirm - Function to show dialog and await user response
 *
 * @example
 * // Basic usage
 * const { isOpen, dialogProps, confirm } = useConfirm();
 *
 * const handleDelete = async () => {
 *   const confirmed = await confirm({
 *     title: 'Delete Chat?',
 *     message: 'This action cannot be undone.',
 *     confirmText: 'Delete',
 *     variant: 'destructive'
 *   });
 *
 *   if (confirmed) {
 *     // User clicked "Delete"
 *     await deleteChat();
 *   } else {
 *     // User clicked "Cancel" or closed dialog
 *   }
 * };
 *
 * return (
 *   <>
 *     <button onClick={handleDelete}>Delete</button>
 *     <ConfirmDialog isOpen={isOpen} {...dialogProps} />
 *   </>
 * );
 *
 * @example
 * // Replacing window.confirm()
 * // OLD:
 * if (window.confirm('Delete this chat?')) {
 *   await deleteChat();
 * }
 *
 * // NEW:
 * if (await confirm({ title: 'Delete Chat?', message: 'This action cannot be undone.' })) {
 *   await deleteChat();
 * }
 */
const useConfirm = () => {
  const [isOpen, setIsOpen] = useState(false);
  const [dialogConfig, setDialogConfig] = useState({
    title: '',
    message: '',
    confirmText: 'Confirm',
    cancelText: 'Cancel',
    variant: 'default'
  });
  const [resolvePromise, setResolvePromise] = useState(null);

  /**
   * Show confirmation dialog and return a promise that resolves
   * to true (confirmed) or false (cancelled)
   */
  const confirm = useCallback((config) => {
    setDialogConfig({
      title: config.title || 'Confirm',
      message: config.message || 'Are you sure?',
      confirmText: config.confirmText || 'Confirm',
      cancelText: config.cancelText || 'Cancel',
      variant: config.variant || 'default'
    });
    setIsOpen(true);

    return new Promise((resolve) => {
      setResolvePromise(() => resolve);
    });
  }, []);

  /**
   * Handle user confirming the action
   */
  const handleConfirm = useCallback(() => {
    setIsOpen(false);
    if (resolvePromise) {
      resolvePromise(true);
    }
  }, [resolvePromise]);

  /**
   * Handle user cancelling the action
   */
  const handleCancel = useCallback(() => {
    setIsOpen(false);
    if (resolvePromise) {
      resolvePromise(false);
    }
  }, [resolvePromise]);

  /**
   * Props to spread onto ConfirmDialog component
   */
  const dialogProps = {
    ...dialogConfig,
    onConfirm: handleConfirm,
    onCancel: handleCancel
  };

  return {
    isOpen,
    dialogProps,
    confirm
  };
};

export default useConfirm;
