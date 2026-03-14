// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useEffect, useRef } from 'react';
import Modal from './Modal';
import { Input } from './ui/input';
import { Button } from './ui/button';

/**
 * CreateKnowledgeItemModal — modal for creating/renaming knowledge files and folders.
 *
 * Props:
 *   show        — whether to display the modal
 *   onClose     — close callback
 *   onSubmit    — called with the trimmed name string
 *   title       — modal title (e.g., "New File", "Rename")
 *   defaultValue — pre-filled value for rename mode
 *   submitLabel — button text (default: "Create")
 */
export default function CreateKnowledgeItemModal({
  show,
  onClose,
  onSubmit,
  title,
  defaultValue = '',
  submitLabel = 'Create',
}) {
  const [name, setName] = useState(defaultValue);
  const inputRef = useRef(null);

  // Reset and focus when modal opens
  useEffect(() => {
    if (show) {
      setName(defaultValue);
      // Focus after render
      requestAnimationFrame(() => {
        inputRef.current?.focus();
        inputRef.current?.select();
      });
    }
  }, [show, defaultValue]);

  const handleSubmit = () => {
    const trimmed = name.trim();
    if (!trimmed) return;
    onSubmit(trimmed);
    onClose();
  };

  const handleKeyDown = (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleSubmit();
    }
  };

  return (
    <Modal
      show={show}
      onClose={onClose}
      title={title}
      size="sm"
      footer={
        <>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={!name.trim()}>
            {submitLabel}
          </Button>
        </>
      }
    >
      <Input
        ref={inputRef}
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder="Enter name..."
      />
    </Modal>
  );
}
