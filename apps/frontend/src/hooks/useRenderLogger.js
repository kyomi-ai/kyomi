// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Development hook to log component re-renders
 * Helps identify performance issues by tracking when components re-render
 */

import { useEffect, useRef } from 'react';

export const useRenderLogger = (componentName, props = {}) => {
  const renderCount = useRef(0);
  const prevProps = useRef(props);

  useEffect(() => {
    renderCount.current += 1;

    // Log changed props if any
    if (renderCount.current > 1) {
      const changedProps = {};
      Object.keys(props).forEach(key => {
        if (prevProps.current[key] !== props[key]) {
          changedProps[key] = {
            prev: prevProps.current[key],
            current: props[key]
          };
        }
      });
    }

    prevProps.current = props;
  });
};

export default useRenderLogger;
