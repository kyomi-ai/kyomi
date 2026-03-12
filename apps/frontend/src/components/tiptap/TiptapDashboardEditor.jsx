// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useEffect, useCallback, useRef, useState, useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { useEditor, EditorContent } from '@tiptap/react';
import StarterKit from '@tiptap/starter-kit';
import Placeholder from '@tiptap/extension-placeholder';
import Link from '@tiptap/extension-link';
import { List, ListOrdered, TextQuote, Code, FileCode, Link2 } from 'lucide-react';
import * as yaml from 'js-yaml';
import { ChartMLNode } from './ChartMLNode';
import { MarkdownRenderer } from '../MarkdownRenderer';
import './tiptap-styles.css';

/**
 * Parse markdown content into Tiptap-compatible JSON structure
 * Converts text, ```chartml blocks, and regular code blocks into appropriate nodes
 */
function parseMarkdownToTiptap(markdown) {
  if (!markdown || !markdown.trim()) {
    return {
      type: 'doc',
      content: [{ type: 'paragraph' }],
    };
  }

  const nodes = [];
  // Match all code blocks: ```language or just ```
  const regex = /```(\w*)\s*\n([\s\S]*?)```/g;
  let lastIndex = 0;
  let match;

  while ((match = regex.exec(markdown)) !== null) {
    // Add text before this code block as paragraphs
    if (match.index > lastIndex) {
      const textContent = markdown.substring(lastIndex, match.index);
      const textNodes = parseTextToParagraphs(textContent);
      nodes.push(...textNodes);
    }

    const language = match[1] || '';
    const content = match[2];

    // Trim trailing newline from code block content (regex captures it before closing ```)
    const trimmedContent = content.replace(/\n$/, '');

    if (language === 'chartml') {
      // ChartML block - parse and split arrays into separate nodes
      try {
        const parsed = yaml.load(trimmedContent);

        if (Array.isArray(parsed)) {
          // Split array into separate chart nodes for easier editing/reordering
          for (const chartSpec of parsed) {
            const singleChartYaml = yaml.dump(chartSpec, {
              indent: 2,
              lineWidth: -1,
              noRefs: true,
            });
            nodes.push({
              type: 'chartMLBlock',
              attrs: {
                content: singleChartYaml.trim(),
                id: `chart-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
              },
            });
          }
        } else {
          // Single chart - keep as is
          nodes.push({
            type: 'chartMLBlock',
            attrs: {
              content: trimmedContent,
              id: `chart-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
            },
          });
        }
      } catch (err) {
        // If YAML parsing fails, keep original content
        nodes.push({
          type: 'chartMLBlock',
          attrs: {
            content: trimmedContent,
            id: `chart-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`,
          },
        });
      }
    } else {
      // Regular code block
      nodes.push({
        type: 'codeBlock',
        attrs: { language },
        content: trimmedContent ? [{ type: 'text', text: trimmedContent }] : [],
      });
    }

    lastIndex = match.index + match[0].length;
  }

  // Add remaining text after last code block
  if (lastIndex < markdown.length) {
    const textContent = markdown.substring(lastIndex);
    const textNodes = parseTextToParagraphs(textContent);
    nodes.push(...textNodes);
  }

  // Ensure we have at least one node
  if (nodes.length === 0) {
    nodes.push({ type: 'paragraph' });
  }

  return {
    type: 'doc',
    content: nodes,
  };
}

/**
 * Parse inline markdown formatting (bold, italic, code, links) into Tiptap marks
 */
function parseInlineMarks(text) {
  if (!text) return [];

  const nodes = [];
  let remaining = text;

  // Process inline formatting patterns
  // Order matters: bold+italic first, then bold, then italic, then code, then links
  const patterns = [
    // Bold + Italic (***text*** or ___text___)
    { regex: /\*\*\*(.+?)\*\*\*|___(.+?)___/, marks: [{ type: 'bold' }, { type: 'italic' }] },
    // Bold (**text** or __text__)
    { regex: /\*\*(.+?)\*\*|__(.+?)__/, marks: [{ type: 'bold' }] },
    // Italic (*text* or _text_)
    { regex: /\*([^*]+?)\*|_([^_]+?)_/, marks: [{ type: 'italic' }] },
    // Inline code (`text`)
    { regex: /`([^`]+?)`/, marks: [{ type: 'code' }] },
    // Links [text](url)
    { regex: /\[([^\]]+)\]\(([^)]+)\)/, isLink: true },
  ];

  while (remaining.length > 0) {
    let earliestMatch = null;
    let earliestIndex = remaining.length;
    let matchedPattern = null;

    // Find the earliest matching pattern
    for (const pattern of patterns) {
      const match = remaining.match(pattern.regex);
      if (match && match.index < earliestIndex) {
        earliestMatch = match;
        earliestIndex = match.index;
        matchedPattern = pattern;
      }
    }

    if (earliestMatch && matchedPattern) {
      // Add plain text before the match
      if (earliestIndex > 0) {
        nodes.push({ type: 'text', text: remaining.substring(0, earliestIndex) });
      }

      // Add the formatted text
      const matchedText = earliestMatch[1] || earliestMatch[2];
      if (matchedPattern.isLink) {
        // Link: [text](url)
        nodes.push({
          type: 'text',
          text: earliestMatch[1],
          marks: [{ type: 'link', attrs: { href: earliestMatch[2] } }],
        });
      } else {
        nodes.push({
          type: 'text',
          text: matchedText,
          marks: matchedPattern.marks,
        });
      }

      remaining = remaining.substring(earliestIndex + earliestMatch[0].length);
    } else {
      // No more patterns found, add remaining text
      nodes.push({ type: 'text', text: remaining });
      break;
    }
  }

  return nodes.filter(n => n.text); // Remove empty text nodes
}

/**
 * Parse text content into paragraph nodes
 * Handles headings, lists, and plain paragraphs with inline formatting
 */
function parseTextToParagraphs(text) {
  const lines = text.split('\n');
  const nodes = [];
  let currentList = null;
  let listType = null;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];

    // Skip empty lines (but may end a list)
    if (!line.trim()) {
      if (currentList) {
        nodes.push(currentList);
        currentList = null;
        listType = null;
      }
      continue;
    }

    // Handle headings
    const headingMatch = line.match(/^(#{1,6})\s+(.+)$/);
    if (headingMatch) {
      if (currentList) {
        nodes.push(currentList);
        currentList = null;
        listType = null;
      }
      const level = headingMatch[1].length;
      const inlineContent = parseInlineMarks(headingMatch[2]);
      nodes.push({
        type: 'heading',
        attrs: { level },
        content: inlineContent.length > 0 ? inlineContent : [{ type: 'text', text: headingMatch[2] }],
      });
      continue;
    }

    // Handle bullet lists
    const bulletMatch = line.match(/^[-*]\s+(.+)$/);
    if (bulletMatch) {
      if (listType !== 'bullet') {
        if (currentList) nodes.push(currentList);
        currentList = { type: 'bulletList', content: [] };
        listType = 'bullet';
      }
      const inlineContent = parseInlineMarks(bulletMatch[1]);
      currentList.content.push({
        type: 'listItem',
        content: [{
          type: 'paragraph',
          content: inlineContent.length > 0 ? inlineContent : [{ type: 'text', text: bulletMatch[1] }],
        }],
      });
      continue;
    }

    // Handle ordered lists
    const orderedMatch = line.match(/^\d+\.\s+(.+)$/);
    if (orderedMatch) {
      if (listType !== 'ordered') {
        if (currentList) nodes.push(currentList);
        currentList = { type: 'orderedList', content: [] };
        listType = 'ordered';
      }
      const inlineContent = parseInlineMarks(orderedMatch[1]);
      currentList.content.push({
        type: 'listItem',
        content: [{
          type: 'paragraph',
          content: inlineContent.length > 0 ? inlineContent : [{ type: 'text', text: orderedMatch[1] }],
        }],
      });
      continue;
    }

    // Handle blockquotes
    const quoteMatch = line.match(/^>\s*(.*)$/);
    if (quoteMatch) {
      if (currentList) {
        nodes.push(currentList);
        currentList = null;
        listType = null;
      }
      const inlineContent = quoteMatch[1] ? parseInlineMarks(quoteMatch[1]) : [];
      nodes.push({
        type: 'blockquote',
        content: [{
          type: 'paragraph',
          content: inlineContent.length > 0 ? inlineContent : (quoteMatch[1] ? [{ type: 'text', text: quoteMatch[1] }] : []),
        }],
      });
      continue;
    }

    // Handle horizontal rules
    if (line.match(/^[-*_]{3,}$/)) {
      if (currentList) {
        nodes.push(currentList);
        currentList = null;
        listType = null;
      }
      nodes.push({ type: 'horizontalRule' });
      continue;
    }

    // Regular paragraph with inline formatting
    if (currentList) {
      nodes.push(currentList);
      currentList = null;
      listType = null;
    }
    const inlineContent = parseInlineMarks(line);
    nodes.push({
      type: 'paragraph',
      content: inlineContent.length > 0 ? inlineContent : (line.trim() ? [{ type: 'text', text: line }] : []),
    });
  }

  // Don't forget any remaining list
  if (currentList) {
    nodes.push(currentList);
  }

  return nodes;
}

/**
 * Serialize Tiptap JSON back to markdown
 * Adjacent chartMLBlock nodes are combined back into a single YAML array for MarkdownRenderer
 */
function serializeTiptapToMarkdown(doc) {
  if (!doc || !doc.content) return '';

  const lines = [];
  let pendingCharts = [];  // Collect adjacent chart nodes to combine into array

  // Helper to flush pending charts as a single code block with YAML array
  const flushCharts = () => {
    if (pendingCharts.length === 0) return;

    lines.push('```chartml');

    if (pendingCharts.length === 1) {
      // Single chart - output as-is
      lines.push(pendingCharts[0]);
    } else {
      // Multiple charts - combine into YAML array
      const charts = pendingCharts.map(content => {
        try {
          return yaml.load(content);
        } catch {
          return null;
        }
      }).filter(Boolean);

      const arrayYaml = yaml.dump(charts, {
        indent: 2,
        lineWidth: -1,
        noRefs: true,
      });
      lines.push(arrayYaml.trim());
    }

    lines.push('```');
    lines.push('');
    pendingCharts = [];
  };

  for (const node of doc.content) {
    switch (node.type) {
      case 'chartMLBlock':
        // Collect chart for potential combining with adjacent charts
        pendingCharts.push(node.attrs.content);
        break;

      case 'heading': {
        flushCharts();  // Output any pending charts before non-chart content
        const hashes = '#'.repeat(node.attrs.level || 1);
        const headingText = getTextContent(node);
        lines.push(`${hashes} ${headingText}`);
        lines.push('');
        break;
      }

      case 'paragraph': {
        flushCharts();  // Output any pending charts before non-chart content
        const paraText = getTextContent(node);
        lines.push(paraText);
        lines.push('');
        break;
      }

      case 'bulletList': {
        flushCharts();  // Output any pending charts before non-chart content
        for (const item of node.content || []) {
          const itemText = getTextContent(item);
          lines.push(`- ${itemText}`);
        }
        lines.push('');
        break;
      }

      case 'orderedList': {
        flushCharts();  // Output any pending charts before non-chart content
        (node.content || []).forEach((item, idx) => {
          const itemText = getTextContent(item);
          lines.push(`${idx + 1}. ${itemText}`);
        });
        lines.push('');
        break;
      }

      case 'blockquote': {
        flushCharts();  // Output any pending charts before non-chart content
        const quoteText = getTextContent(node);
        lines.push(`> ${quoteText}`);
        lines.push('');
        break;
      }

      case 'horizontalRule':
        flushCharts();  // Output any pending charts before non-chart content
        lines.push('---');
        lines.push('');
        break;

      case 'codeBlock': {
        flushCharts();  // Output any pending charts before non-chart content
        const lang = node.attrs?.language || '';
        lines.push('```' + lang);
        lines.push(getTextContent(node));
        lines.push('```');
        lines.push('');
        break;
      }

      default: {
        flushCharts();  // Output any pending charts before non-chart content
        // Unknown node type - try to extract text
        const text = getTextContent(node);
        if (text) {
          lines.push(text);
          lines.push('');
        }
      }
    }
  }

  // Flush any remaining charts at end of document
  flushCharts();

  // Clean up extra blank lines
  return lines.join('\n').replace(/\n{3,}/g, '\n\n').trim();
}

/**
 * Extract text content from a node, preserving markdown formatting
 */
function getTextContent(node) {
  if (!node) return '';

  if (node.type === 'text') {
    let text = node.text || '';

    // Apply marks as markdown
    if (node.marks && node.marks.length > 0) {
      for (const mark of node.marks) {
        switch (mark.type) {
          case 'bold':
            text = `**${text}**`;
            break;
          case 'italic':
            text = `*${text}*`;
            break;
          case 'code':
            text = `\`${text}\``;
            break;
          case 'link':
            text = `[${text}](${mark.attrs?.href || ''})`;
            break;
        }
      }
    }

    return text;
  }

  if (node.content) {
    return node.content.map(getTextContent).join('');
  }

  return '';
}

/**
 * TiptapDashboardEditor - Unified block editor for dashboards
 *
 * Features:
 * - Rich text editing with Tiptap/ProseMirror
 * - ChartML blocks rendered inline as interactive charts
 * - Insert charts at cursor position
 * - Seamless markdown serialization/deserialization
 */
export const TiptapDashboardEditor = React.forwardRef(function TiptapDashboardEditor({
  content,
  onChange,
  onEditChart,
  onInsertChart,  // Callback to open chart builder for new chart insertion
  onInsertDashboardLink,  // Callback to open dashboard link picker modal
  onSaveChartToDashboard,  // Callback for "Add to Dashboard" button on charts
  onShowChartInfo,  // Callback for "Chart Info" button on charts
  onAskAboutChart,  // Callback for "Ask about this chart" button - navigates to chat
  placeholder = 'Start typing...',
  rightSlot,  // Optional content to render on the right side of the toolbar
  readOnly = false,  // When true, editor is non-editable (for version preview)
  previewLabel = null,  // Optional label to show in toolbar when in preview mode (e.g., "Version 3")
}, ref) {
  const navigate = useNavigate();
  const isUpdatingFromProps = useRef(false);
  const lastMarkdown = useRef(content);
  const savedSelectionRef = useRef(null);  // Store cursor position for chart insertion
  const editorContainerRef = useRef(null);  // Ref for editor container to intercept link clicks

  // Force re-render when selection changes so toolbar updates
  const [, setSelectionUpdate] = useState(0);

  // Memoize extensions to prevent editor recreation on re-render
  const extensions = useMemo(() => [
    StarterKit.configure({
      // CodeBlock enabled for regular code blocks (```js, ```sql, etc.)
      // ChartMLNode handles ```chartml blocks separately
      // Disable Link here - we'll add it with custom config below to avoid duplicate extension warning
      Link: false,
    }),
    Placeholder.configure({
      placeholder,
    }),
    Link.configure({
      openOnClick: false,
      HTMLAttributes: {
        class: 'text-primary underline',
      },
    }),
    ChartMLNode,
  ], [placeholder]);

  // Memoize initial content - only computed once, updates handled via useEffect
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const initialContent = useMemo(() => parseMarkdownToTiptap(content), []);

  // Memoize editorProps
  const editorProps = useMemo(() => ({
    attributes: {
      class: 'tiptap-content focus:outline-none min-h-[200px]',
    },
  }), []);

  // Memoize callbacks to prevent editor recreation
  const handleUpdate = useCallback(({ editor }) => {
    // Don't trigger onChange if we're updating from props
    if (isUpdatingFromProps.current) return;

    const json = editor.getJSON();
    const markdown = serializeTiptapToMarkdown(json);

    // Only call onChange if content actually changed
    if (markdown !== lastMarkdown.current) {
      lastMarkdown.current = markdown;
      onChange?.(markdown);
    }
  }, [onChange]);

  const handleSelectionUpdate = useCallback(() => {
    // Force re-render to update toolbar active states
    setSelectionUpdate(n => n + 1);
  }, []);

  // Initialize Tiptap editor with memoized options
  const editor = useEditor({
    extensions,
    content: initialContent,
    // Note: editable is controlled via useEffect to avoid editor recreation on readOnly change
    onUpdate: handleUpdate,
    onSelectionUpdate: handleSelectionUpdate,
    editorProps,
  });

  // Update editable state when readOnly prop changes
  useEffect(() => {
    if (editor) {
      editor.setEditable(!readOnly);
    }
  }, [editor, readOnly]);

  // Update editor when content prop changes (external update)
  useEffect(() => {
    if (!editor) return;
    if (content === lastMarkdown.current) return;

    isUpdatingFromProps.current = true;
    lastMarkdown.current = content;

    const newContent = parseMarkdownToTiptap(content);
    editor.commands.setContent(newContent);

    // Reset flag after a tick
    setTimeout(() => {
      isUpdatingFromProps.current = false;
    }, 0);
  }, [content, editor]);

  // Listen for chart edit events from ChartMLNode
  useEffect(() => {
    const handleEditChart = (event) => {
      const { spec, chartIndex, arrayIndex, nodeId, yamlContent } = event.detail;
      onEditChart?.(spec, chartIndex, arrayIndex, nodeId, yamlContent);
    };

    window.addEventListener('tiptap-edit-chart', handleEditChart);
    return () => window.removeEventListener('tiptap-edit-chart', handleEditChart);
  }, [onEditChart]);

  // Listen for "Add to Dashboard" events from ChartMLNode
  useEffect(() => {
    const handleSaveChart = (event) => {
      const { chartMarkdown } = event.detail;
      onSaveChartToDashboard?.(chartMarkdown);
    };

    window.addEventListener('tiptap-save-chart-to-dashboard', handleSaveChart);
    return () => window.removeEventListener('tiptap-save-chart-to-dashboard', handleSaveChart);
  }, [onSaveChartToDashboard]);

  // Listen for "Chart Info" events from ChartMLNode
  useEffect(() => {
    const handleShowInfo = (event) => {
      const { spec } = event.detail;
      onShowChartInfo?.(spec);
    };

    window.addEventListener('tiptap-show-chart-info', handleShowInfo);
    return () => window.removeEventListener('tiptap-show-chart-info', handleShowInfo);
  }, [onShowChartInfo]);

  // Listen for "Ask about this chart" events from ChartMLNode
  useEffect(() => {
    const handleAskAbout = (event) => {
      const { chartMarkdown, spec } = event.detail;
      onAskAboutChart?.(chartMarkdown, spec);
    };

    window.addEventListener('tiptap-ask-about-chart', handleAskAbout);
    return () => window.removeEventListener('tiptap-ask-about-chart', handleAskAbout);
  }, [onAskAboutChart]);

  // Intercept clicks on internal links to use React Router navigation
  useEffect(() => {
    const container = editorContainerRef.current;
    if (!container) return;

    const handleLinkClick = (e) => {
      // Find if the click was on a link or inside a link
      const link = e.target.closest('a');
      if (!link) return;

      const href = link.getAttribute('href');
      if (!href) return;

      // Check if it's an internal link (starts with /)
      const isInternal = href.startsWith('/');
      const isExternal = href.startsWith('http://') || href.startsWith('https://');

      if (isInternal) {
        e.preventDefault();
        navigate(href);
      } else if (!isExternal) {
        // Relative paths - also use navigate
        e.preventDefault();
        navigate(href);
      }
      // External links will open normally (or in new tab if target="_blank")
    };

    container.addEventListener('click', handleLinkClick);
    return () => container.removeEventListener('click', handleLinkClick);
  }, [navigate]);

  // Save cursor position and open chart builder
  const handleAddChartClick = useCallback(() => {
    if (!editor) return;

    // Save current selection/cursor position
    savedSelectionRef.current = editor.state.selection;

    // Call parent to open chart builder
    if (onInsertChart) {
      onInsertChart();
    }
  }, [editor, onInsertChart]);

  // Save cursor position and open dashboard link picker
  const handleInsertDashboardLinkClick = useCallback(() => {
    if (!editor) return;

    // Save current selection/cursor position
    savedSelectionRef.current = editor.state.selection;

    // Call parent to open dashboard link picker modal
    if (onInsertDashboardLink) {
      onInsertDashboardLink();
    }
  }, [editor, onInsertDashboardLink]);

  // Insert chart at saved cursor position (called after chart builder saves)
  const insertChartAtSavedPosition = useCallback((chartYaml) => {
    if (!editor || !chartYaml) return;

    // Restore selection if we have one saved
    if (savedSelectionRef.current) {
      editor.chain().focus().setTextSelection(savedSelectionRef.current).run();
    }

    // Insert the chart
    editor.chain().focus().insertChartML(chartYaml).run();

    // Clear saved selection
    savedSelectionRef.current = null;
  }, [editor]);

  // Insert a dashboard link at saved cursor position
  const insertLinkAtSavedPosition = useCallback((title, dashboardId) => {
    if (!editor || !title || !dashboardId) return;

    // Restore selection if we have one saved
    if (savedSelectionRef.current) {
      editor.chain().focus().setTextSelection(savedSelectionRef.current).run();
    }

    // Insert the link using Tiptap's link mark
    const url = `/dashboard/${dashboardId}`;
    editor
      .chain()
      .focus()
      .insertContent({
        type: 'text',
        text: title,
        marks: [{ type: 'link', attrs: { href: url } }],
      })
      .run();

    // Clear saved selection
    savedSelectionRef.current = null;
  }, [editor]);

  // Direct insert (for programmatic use without chart builder)
  const insertChart = useCallback((chartYaml = '') => {
    if (!editor) return;

    const defaultChart = chartYaml || `source:
  type: inline
  data:
    - month: Jan
      value: 100
    - month: Feb
      value: 150
    - month: Mar
      value: 200

visualize:
  type: bar
  columns: month
  rows: value
  style:
    title: "My Chart"
`;

    editor.chain().focus().insertChartML(defaultChart).run();
  }, [editor]);

  // Update a specific chart by ID
  const updateChart = useCallback((nodeId, newYaml) => {
    if (!editor) return;
    editor.commands.updateChartML(nodeId, newYaml);
  }, [editor]);

  // Expose methods for parent component via ref
  React.useImperativeHandle(ref, () => ({
    insertChart,
    insertChartAtSavedPosition,
    insertLinkAtSavedPosition,
    updateChart,
    getEditor: () => editor,
  }), [editor, insertChart, insertChartAtSavedPosition, insertLinkAtSavedPosition, updateChart]);

  if (!editor) {
    return (
      <div className="p-4 text-muted-foreground">
        Loading editor...
      </div>
    );
  }

  return (
    <div className="tiptap-dashboard-editor flex flex-col h-full">
      {/* Toolbar - changes based on readOnly mode */}
      <div className={`flex items-center gap-1 p-3 border-b flex-shrink-0 ${
        readOnly
          ? 'border-warning-border bg-warning'
          : 'border-border bg-muted/50 overflow-x-auto scrollbar-thin scrollbar-thumb-border scrollbar-track-transparent'
      }`}>
        {readOnly ? (
          /* Read-only toolbar: just preview label and mode toggle */
          <>
            <span className="text-sm font-medium text-warning-foreground">
              {previewLabel ? `Previewing ${previewLabel}` : 'Preview'}
            </span>
            <span className="text-xs text-warning-foreground/70">Read-only</span>
            <div className="flex-1" />
            {rightSlot && <div className="flex-shrink-0">{rightSlot}</div>}
          </>
        ) : (
          /* Edit toolbar: all formatting buttons */
          <>
        <button
          type="button"
          onClick={() => editor.chain().focus().toggleHeading({ level: 1 }).run()}
          className={`px-2 py-1 text-xs rounded ${
            editor.isActive('heading', { level: 1 }) ? 'bg-primary text-primary-foreground' : 'text-foreground hover:bg-accent'
          }`}
        >
          H1
        </button>
        <button
          type="button"
          onClick={() => editor.chain().focus().toggleHeading({ level: 2 }).run()}
          className={`px-2 py-1 text-xs rounded ${
            editor.isActive('heading', { level: 2 }) ? 'bg-primary text-primary-foreground' : 'text-foreground hover:bg-accent'
          }`}
        >
          H2
        </button>
        <button
          type="button"
          onClick={() => editor.chain().focus().toggleHeading({ level: 3 }).run()}
          className={`px-2 py-1 text-xs rounded ${
            editor.isActive('heading', { level: 3 }) ? 'bg-primary text-primary-foreground' : 'text-foreground hover:bg-accent'
          }`}
        >
          H3
        </button>
        <div className="w-px h-4 bg-border mx-1 flex-shrink-0" />
        <button
          type="button"
          onClick={() => editor.chain().focus().toggleBold().run()}
          className={`px-2 py-1 text-xs rounded font-bold ${
            editor.isActive('bold') ? 'bg-primary text-primary-foreground' : 'text-foreground hover:bg-accent'
          }`}
        >
          B
        </button>
        <button
          type="button"
          onClick={() => editor.chain().focus().toggleItalic().run()}
          className={`px-2 py-1 text-xs rounded italic ${
            editor.isActive('italic') ? 'bg-primary text-primary-foreground' : 'text-foreground hover:bg-accent'
          }`}
        >
          I
        </button>
        <button
          type="button"
          onClick={() => editor.chain().focus().toggleCode().run()}
          className={`p-1.5 rounded ${
            editor.isActive('code') ? 'bg-primary text-primary-foreground' : 'text-foreground hover:bg-accent'
          }`}
          title="Inline code"
        >
          <Code className="w-4 h-4" />
        </button>
        <div className="w-px h-4 bg-border mx-1 flex-shrink-0" />
        <button
          type="button"
          onClick={() => editor.chain().focus().toggleBulletList().run()}
          className={`p-1.5 rounded ${
            editor.isActive('bulletList') ? 'bg-primary text-primary-foreground' : 'text-foreground hover:bg-accent'
          }`}
          title="Bullet list"
        >
          <List className="w-4 h-4" />
        </button>
        <button
          type="button"
          onClick={() => editor.chain().focus().toggleOrderedList().run()}
          className={`p-1.5 rounded ${
            editor.isActive('orderedList') ? 'bg-primary text-primary-foreground' : 'text-foreground hover:bg-accent'
          }`}
          title="Numbered list"
        >
          <ListOrdered className="w-4 h-4" />
        </button>
        <button
          type="button"
          onClick={() => editor.chain().focus().toggleBlockquote().run()}
          className={`p-1.5 rounded ${
            editor.isActive('blockquote') ? 'bg-primary text-primary-foreground' : 'text-foreground hover:bg-accent'
          }`}
          title="Quote"
        >
          <TextQuote className="w-4 h-4" />
        </button>
        <button
          type="button"
          onClick={() => editor.chain().focus().toggleCodeBlock().run()}
          className={`p-1.5 rounded ${
            editor.isActive('codeBlock') ? 'bg-primary text-primary-foreground' : 'text-foreground hover:bg-accent'
          }`}
          title="Code block"
        >
          <FileCode className="w-4 h-4" />
        </button>
        {/* Add Chart button - only show when onInsertChart is provided */}
        {onInsertChart && (
          <>
            <div className="w-px h-4 bg-border mx-1 flex-shrink-0" />
            <button
              type="button"
              onClick={handleAddChartClick}
              className="p-1.5 sm:px-2 sm:py-1 text-xs rounded bg-secondary text-secondary-foreground hover:bg-secondary/90 flex items-center gap-1 flex-shrink-0"
              title="Add Chart"
            >
              <svg className="w-4 h-4 sm:w-3 sm:h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
              </svg>
              <span className="hidden sm:inline">Add Chart</span>
            </button>
          </>
        )}
        {/* Link to Dashboard button - only show when onInsertDashboardLink is provided */}
        {onInsertDashboardLink && (
          <button
            type="button"
            onClick={handleInsertDashboardLinkClick}
            className="p-1.5 sm:px-2 sm:py-1 text-xs rounded text-foreground hover:bg-accent flex items-center gap-1 flex-shrink-0"
            title="Link to Dashboard"
          >
            <Link2 className="w-4 h-4 sm:w-3 sm:h-3" />
            <span className="hidden sm:inline">Link</span>
          </button>
        )}
        {/* Spacer to push rightSlot to the right */}
        {rightSlot && <div className="flex-1 min-w-4" />}
        {/* Right slot for custom content (e.g., mode toggle) */}
        {rightSlot && <div className="flex-shrink-0">{rightSlot}</div>}
          </>
        )}
      </div>

      {/* Content area - MarkdownRenderer in readOnly mode, EditorContent otherwise */}
      <div ref={editorContainerRef} className="flex-1 overflow-auto p-4 md:p-6">
        {readOnly ? (
          <MarkdownRenderer>{content}</MarkdownRenderer>
        ) : (
          <EditorContent editor={editor} className="min-h-[300px]" />
        )}
      </div>
    </div>
  );
});

export default TiptapDashboardEditor;
