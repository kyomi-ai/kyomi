// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Hashing utility for pipeline table naming.
 *
 * Shared between the pipeline runner and individual stages.
 * NOT a security hash — used for generating deterministic, unique table identifiers.
 *
 * Uses a DJB2-variant for fast, deterministic hashing. Pure JS, works in both
 * browser and Node.js without any dependencies.
 *
 * @module hash
 */

/**
 * Generate a short hash string from input text (synchronous).
 * Uses a DJB2-variant for fast, deterministic hashing.
 *
 * @param {string} input - String to hash
 * @returns {string} 16-character hex hash
 */
export function hash(input) {
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
