// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState, useRef, useEffect } from 'react';
import { PencilIcon, CheckIcon, XMarkIcon } from '@heroicons/react/24/outline';
import { Button } from './ui/button';

/**
 * InlineEditableTitle - A consistent inline title editing component
 *
 * Features:
 * - Click to edit (or use edit button)
 * - Shows pencil icon on hover
 * - Save/cancel buttons with Heroicons
 * - Semantic design tokens throughout
 * - Handles empty/placeholder states
 *
 * Usage:
 * <InlineEditableTitle
 *   value={title}
 *   onSave={(newTitle) => updateTitle(newTitle)}
 *   placeholder="Untitled"
 * />
 */
const InlineEditableTitle = ({
  value = '',
  onSave,
  placeholder = 'Untitled',
  className = ''
}) => {
  const [isEditing, setIsEditing] = useState(false);
  const [editValue, setEditValue] = useState(value);
  const inputRef = useRef(null);

  // Update editValue when value prop changes
  useEffect(() => {
    setEditValue(value);
  }, [value]);

  // Focus input when entering edit mode
  useEffect(() => {
    if (isEditing && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [isEditing]);

  const handleSave = () => {
    const trimmedValue = editValue.trim();
    if (trimmedValue && trimmedValue !== value) {
      onSave(trimmedValue);
    }
    setIsEditing(false);
  };

  const handleCancel = () => {
    setEditValue(value);
    setIsEditing(false);
  };

  const handleKeyDown = (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleSave();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      handleCancel();
    }
  };

  if (isEditing) {
    return (
      <div className={`flex items-center gap-2 ${className}`}>
        <input
          ref={inputRef}
          type="text"
          value={editValue}
          onChange={(e) => setEditValue(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={placeholder}
          className="text-base font-semibold px-2 py-1 border-0 border-b-2 border-b-transparent bg-transparent focus:outline-none focus:border-b-ring text-foreground min-w-0 flex-1 transition-colors"
        />
        <Button
          variant="ghost"
          size="icon"
          className="h-7 w-7 flex-shrink-0"
          onClick={handleSave}
        >
          <CheckIcon className="h-4 w-4" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          className="h-7 w-7 flex-shrink-0"
          onClick={handleCancel}
        >
          <XMarkIcon className="h-4 w-4" />
        </Button>
      </div>
    );
  }

  return (
    <button
      onClick={() => setIsEditing(true)}
      className={`flex items-center gap-2 group hover:bg-accent/50 rounded px-2 py-1 transition-colors min-w-0 ${className}`}
    >
      <span className="text-base font-semibold text-foreground truncate">
        {value || placeholder}
      </span>
      <PencilIcon className="h-4 w-4 text-muted-foreground opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0" />
    </button>
  );
};

export default InlineEditableTitle;
