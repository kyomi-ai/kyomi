// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState } from 'react';
import { ClipboardDocumentIcon, CheckIcon } from '@heroicons/react/24/outline';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import { oneLight, oneDark } from 'react-syntax-highlighter/dist/esm/styles/prism';
import { useTheme } from '../context/ThemeContext';
import Modal from './Modal';
import { serializeChart } from '../utils/chartParser';

/**
 * CopyableCodeBlock - Code block with floating copy button on hover
 */
function CopyableCodeBlock({ code, language, label }) {
  const [copied, setCopied] = useState(false);
  const { resolvedTheme } = useTheme();

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(code);
    } catch {
      // Fallback for non-secure contexts (HTTP, non-localhost)
      const textarea = document.createElement('textarea');
      textarea.value = code;
      textarea.style.position = 'fixed';
      textarea.style.opacity = '0';
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand('copy');
      document.body.removeChild(textarea);
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="space-y-2">
      <span className="text-sm font-medium text-foreground">{label}</span>
      <div className="relative group">
        <button
          onClick={handleCopy}
          className="absolute top-2 right-2 p-2 rounded bg-accent hover:bg-accent/80 opacity-0 group-hover:opacity-100 transition-opacity z-10"
          title={copied ? 'Copied!' : 'Copy code'}
        >
          {copied ? (
            <CheckIcon className="h-4 w-4 text-success-foreground" />
          ) : (
            <ClipboardDocumentIcon className="h-4 w-4 text-muted-foreground" />
          )}
        </button>
        <SyntaxHighlighter
          language={language}
          style={resolvedTheme === 'dark' ? oneDark : oneLight}
          showLineNumbers={false}
          customStyle={{
            margin: 0,
            borderRadius: '6px',
            border: 'none',
            fontSize: '0.929rem',
            backgroundColor: 'var(--color-muted)',
            padding: '16px',
            maxHeight: '300px',
            overflow: 'auto',
          }}
          codeTagProps={{
            style: {
              fontFamily: 'var(--font-mono)',
              backgroundColor: 'transparent',
            }
          }}
        >
          {code}
        </SyntaxHighlighter>
      </div>
    </div>
  );
}

/**
 * ChartInfoModal - Shows chart source information
 *
 * Displays datasource, SQL query, and full ChartML source code
 * with copy buttons for each section.
 *
 * @param {boolean} isOpen - Whether modal is visible
 * @param {function} onClose - Callback to close modal
 * @param {object} spec - ChartML spec object
 */
export default function ChartInfoModal({ isOpen, onClose, spec }) {
  if (!spec) return null;

  // Extract datasource from spec
  const datasource = spec.data?.datasource || spec.data?.source || 'Not specified';

  // Extract SQL query from spec
  const query = spec.data?.query || null;

  // Serialize full spec to YAML
  const chartYaml = serializeChart(spec);

  return (
    <Modal
      show={isOpen}
      onClose={onClose}
      title="Chart Info"
      size="lg"
    >
      <div className="space-y-6">
        {/* Datasource */}
        <div>
          <span className="text-sm font-medium text-foreground">Datasource</span>
          <p className="mt-1 text-sm text-muted-foreground font-mono bg-muted px-3 py-2 rounded-md">
            {datasource}
          </p>
        </div>

        {/* SQL Query */}
        {query && (
          <CopyableCodeBlock
            code={query.trim()}
            language="sql"
            label="SQL Query"
          />
        )}

        {/* ChartML Source */}
        <CopyableCodeBlock
          code={chartYaml}
          language="yaml"
          label="ChartML Source"
        />
      </div>
    </Modal>
  );
}
