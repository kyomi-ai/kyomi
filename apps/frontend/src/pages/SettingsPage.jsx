// SPDX-License-Identifier: AGPL-3.0-or-later
import React from 'react';
import { useNavigate } from 'react-router-dom';
import SettingsContent from './SettingsContent';
import { Tooltip, TooltipContent, TooltipTrigger } from '../components/ui/tooltip';

const SettingsPage = () => {
  const navigate = useNavigate();

  return (
    <div className="flex flex-col h-full bg-muted overflow-x-hidden" style={{flexDirection: 'column'}}>
      <div className="flex-1 overflow-y-auto p-4 md:p-6 relative">
        <div className="w-full">
          {/* Close button in top-right */}
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                onClick={() => navigate('/')}
                className="absolute top-4 right-4 md:top-6 md:right-6 p-2 text-muted-foreground hover:text-foreground hover:bg-accent rounded-lg transition-colors z-10"
                aria-label="Close settings"
              >
                <svg className="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                </svg>
              </button>
            </TooltipTrigger>
            <TooltipContent>Close settings</TooltipContent>
          </Tooltip>
          <SettingsContent />
        </div>
      </div>
    </div>
  );
};

export default SettingsPage;
