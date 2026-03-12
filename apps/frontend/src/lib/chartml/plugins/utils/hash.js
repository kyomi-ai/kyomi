// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Hashing utilities for pipeline table naming.
 *
 * Shared between the pipeline runner and individual stages.
 * NOT a security hash — used for generating deterministic, unique table identifiers.
 *
 * @module hash
 */

/**
 * Generate SHA-256 hash of input string (async).
 * Used by the pipeline runner for cache table IDs.
 *
 * @param {string} input - String to hash
 * @returns {Promise<string>} 64-character hex hash
 */
export async function hashAsync(input) {
  // crypto.subtle is only available in secure contexts (HTTPS or localhost).
  // Fall back to the synchronous DJB2 hash for non-secure contexts (e.g. LAN IP).
  if (typeof crypto === 'undefined' || !crypto.subtle) {
    return hashSync(input);
  }
  const encoder = new TextEncoder();
  const data = encoder.encode(input);
  const hashBuffer = await crypto.subtle.digest('SHA-256', data);
  const hashArray = Array.from(new Uint8Array(hashBuffer));
  return hashArray.map(b => b.toString(16).padStart(2, '0')).join('');
}

/**
 * Generate a short hash string from input text (synchronous).
 * Used by stages for intermediate table naming where async is unnecessary overhead.
 * Uses a DJB2-variant for fast, deterministic hashing.
 *
 * @param {string} input - String to hash
 * @returns {string} 16-character hex hash
 */
export function hashSync(input) {
  let h1 = 0xdeadbeef;
  let h2 = 0x41c6ce57;
  for (let i = 0; i < input.length; i++) {
    const ch = input.charCodeAt(i);
    h1 = Math.imul(h1 ^ ch, 2654435761);
    h2 = Math.imul(h2 ^ ch, 1597334677);
  }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507);
  h1 ^= Math.imul(h2 ^ (h2 >>> 13), 3266489909);
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507);
  h2 ^= Math.imul(h1 ^ (h1 >>> 13), 3266489909);
  const combined = 4294967296 * (2097151 & h2) + (h1 >>> 0);
  return combined.toString(16).padStart(16, '0');
}
