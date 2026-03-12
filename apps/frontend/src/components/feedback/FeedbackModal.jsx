// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Feedback Modal Component
 *
 * Modal dialog for submitting user feedback with:
 * - Feedback type selection (Bug, Feature, Question)
 * - Description textarea
 * - Optional screenshot attachment (capture current screen or upload)
 * - Consent checkbox for including technical context
 */

import { useState } from 'react';
import { Bug, Lightbulb, HelpCircle, Camera, Upload, X } from 'lucide-react';
import { Spinner } from '../ui/spinner';
import { Button } from '../ui/button';
import Modal from '../Modal';
import { useAuth } from '../../context/AuthContext';
import { feedbackContext } from '../../lib/feedbackContext';
import { toast } from 'sonner';
import apiClient from '../../api/apiClient';

const FEEDBACK_TYPES = [
  { value: 'bug', label: 'Bug', icon: Bug },
  { value: 'feature', label: 'Feature', icon: Lightbulb },
  { value: 'question', label: 'Question', icon: HelpCircle },
];

export default function FeedbackModal({ open, onOpenChange }) {
  const { user } = useAuth();

  const [type, setType] = useState('bug');
  const [description, setDescription] = useState('');
  const [screenshot, setScreenshot] = useState(null);
  const [includeContext, setIncludeContext] = useState(true);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [isCapturing, setIsCapturing] = useState(false);

  const handleSubmit = async () => {
    if (!description.trim()) {
      toast.error('Please describe your feedback');
      return;
    }

    if (description.trim().length < 10) {
      toast.error('Please provide more detail (at least 10 characters)');
      return;
    }

    setIsSubmitting(true);
    try {
      const payload = {
        type,
        description: description.trim(),
        screenshot: screenshot, // base64 string or null
        include_context: includeContext,
        context: includeContext ? feedbackContext.getContext() : null,
        workspace_id: user?.workspace_id || null,
      };

      await apiClient.post('/api/v1/feedback', payload);

      toast.success('Thank you! Feedback like yours helps shape Kyomi 🙏');
      feedbackContext.clear();
      resetForm();
      onOpenChange(false);
    } catch (error) {
      const errorMessage = error.response?.data?.detail || 'Failed to send feedback. Please try again.';
      toast.error(errorMessage);
    } finally {
      setIsSubmitting(false);
    }
  };

  const resetForm = () => {
    setType('bug');
    setDescription('');
    setScreenshot(null);
    setIncludeContext(true);
  };

  const handleClose = () => {
    if (!isSubmitting) {
      onOpenChange(false);
    }
  };

  const captureScreen = async () => {
    setIsCapturing(true);

    // Temporarily hide the modal to capture what's behind it
    onOpenChange(false);

    // Small delay to let the modal close animation complete
    await new Promise(resolve => setTimeout(resolve, 150));

    try {
      // Use native browser screen capture API (handles all modern CSS including oklch)
      const stream = await navigator.mediaDevices.getDisplayMedia({
        video: {
          displaySurface: 'browser',
        },
        preferCurrentTab: true,
      });

      // Get video track and capture a frame
      const track = stream.getVideoTracks()[0];
      const imageCapture = new ImageCapture(track);
      const bitmap = await imageCapture.grabFrame();

      // Stop the stream immediately after capture
      stream.getTracks().forEach(t => t.stop());

      // Convert to canvas then to data URL
      const canvas = document.createElement('canvas');
      canvas.width = bitmap.width;
      canvas.height = bitmap.height;
      const ctx = canvas.getContext('2d');
      ctx.drawImage(bitmap, 0, 0);

      const dataUrl = canvas.toDataURL('image/png');
      setScreenshot(dataUrl);
      toast.success('Screen captured!');
    } catch (error) {
      if (error.name === 'NotAllowedError') {
        toast.error('Screen capture cancelled');
      } else {
        toast.error('Screen capture failed. Try "Upload Image" instead.');
      }
    } finally {
      // Reopen the modal
      onOpenChange(true);
      setIsCapturing(false);
    }
  };

  const showFilePicker = () => {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = 'image/*';
    input.onchange = (e) => {
      const file = e.target.files[0];
      if (file) {
        // Validate file size (max 5MB)
        if (file.size > 5 * 1024 * 1024) {
          toast.error('Screenshot must be less than 5MB');
          return;
        }
        const reader = new FileReader();
        reader.onload = (ev) => setScreenshot(ev.target.result);
        reader.readAsDataURL(file);
      }
    };
    input.click();
  };

  const getPlaceholder = () => {
    switch (type) {
      case 'bug':
        return 'What happened? What did you expect to happen?';
      case 'feature':
        return 'What would you like to see? How would it help you?';
      case 'question':
        return "What's your question? What are you trying to do?";
      default:
        return 'Describe your feedback...';
    }
  };

  return (
    <Modal
      show={open}
      onClose={handleClose}
      title="Send Feedback"
      size="md"
      footer={
        <>
          <Button variant="ghost" onClick={handleClose} disabled={isSubmitting}>
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={isSubmitting}>
            {isSubmitting && <Spinner size="sm" className="mr-2" />}
            Send Feedback
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        {/* Feedback Type Toggle */}
        <div className="space-y-2">
          <label className="text-sm font-medium text-foreground">What type of feedback?</label>
          <div className="flex gap-2">
            {FEEDBACK_TYPES.map((feedbackType) => {
              const TypeIcon = feedbackType.icon;
              return (
                <Button
                  key={feedbackType.value}
                  type="button"
                  variant={type === feedbackType.value ? 'default' : 'outline'}
                  size="sm"
                  onClick={() => setType(feedbackType.value)}
                  className="gap-2"
                >
                  <TypeIcon className="h-4 w-4" />
                  {feedbackType.label}
                </Button>
              );
            })}
          </div>
        </div>

        {/* Description */}
        <div className="space-y-2">
          <label className="text-sm font-medium text-foreground" htmlFor="feedback-description">
            Description
          </label>
          <textarea
            id="feedback-description"
            placeholder={getPlaceholder()}
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            rows={4}
            className="flex w-full rounded-md border border-input bg-transparent px-3 py-2 text-sm shadow-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 resize-none"
          />
        </div>

        {/* Screenshot */}
        <div className="space-y-2">
          <label className="text-sm font-medium text-foreground">Screenshot (optional)</label>
          {screenshot ? (
            <div className="relative inline-block">
              <img
                src={screenshot}
                alt="Screenshot preview"
                className="max-h-32 rounded border border-border"
              />
              <Button
                variant="ghost"
                size="sm"
                className="absolute top-1 right-1 h-6 w-6 p-0 bg-background/80 hover:bg-background"
                onClick={() => setScreenshot(null)}
                aria-label="Remove screenshot"
              >
                <X className="h-4 w-4" />
              </Button>
            </div>
          ) : (
            <div className="flex gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={captureScreen}
                disabled={isCapturing}
                className="gap-2"
                type="button"
              >
                {isCapturing ? (
                  <Spinner size="sm" />
                ) : (
                  <Camera className="h-4 w-4" />
                )}
                Capture Screen
              </Button>
              <Button
                variant="outline"
                size="sm"
                onClick={showFilePicker}
                className="gap-2"
                type="button"
              >
                <Upload className="h-4 w-4" />
                Upload Image
              </Button>
            </div>
          )}
        </div>

        {/* Context Consent */}
        <div className="flex items-start space-x-3 rounded-md border border-border p-3 bg-muted/50">
          <input
            type="checkbox"
            id="include-context"
            checked={includeContext}
            onChange={(e) => setIncludeContext(e.target.checked)}
            className="mt-1 h-4 w-4 rounded border-input text-primary focus:ring-ring"
          />
          <div className="space-y-1">
            <label
              htmlFor="include-context"
              className="text-sm font-medium cursor-pointer text-foreground"
            >
              Include technical details to help us debug faster
            </label>
            <p className="text-xs text-muted-foreground">
              Current page, browser info, and recent errors
            </p>
          </div>
        </div>
      </div>
    </Modal>
  );
}
