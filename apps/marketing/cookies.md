---
layout: doc
title: Cookie Policy
description: How Kyomi uses cookies
---

# Cookie Policy

**Last Updated:** February 21, 2026

## The Short Version

We use minimal cookies, and only essential ones:

- ✅ **Authentication cookies** - Keep you logged in
- ❌ **No tracking cookies** - We don't track you across websites
- ❌ **No advertising cookies** - We don't show ads
- ❌ **No analytics cookies** — Our analytics is completely cookie-free

**You don't need to accept a cookie banner** because we only use essential cookies that are required for the service to work, and our analytics are completely cookie-free.

---

## What Are Cookies?

Cookies are small text files stored by your browser. They help websites remember information about your visit.

**Types of cookies:**
- **Session cookies** - Deleted when you close your browser
- **Persistent cookies** - Stay on your device for a set period
- **First-party cookies** - Set by the website you're visiting (us)
- **Third-party cookies** - Set by other domains (we don't use these)

---

## Cookies We Use

### Essential Cookies (Required)

These cookies are necessary for Kyomi to work. You can't opt out without breaking the service.

| Cookie Name | Purpose | Type |
|-------------|---------|------|
| `access_token` | Authentication - keeps you logged in | First-party, HTTPOnly, Secure, SameSite=Strict |
| `refresh_token` | Renews your session automatically | First-party, HTTPOnly, Secure, SameSite=Strict |

**Security features:**
- `HTTPOnly` - JavaScript can't access these cookies (prevents XSS attacks)
- `Secure` - Only sent over HTTPS connections
- `SameSite=Strict` - Prevents CSRF attacks

### Website Analytics (Cookie-Free)

We use our own cookie-free analytics on kyomi.ai and app.kyomi.ai:

| Service | Purpose | Cookies? | Privacy |
|---------|---------|----------|---------|
| Kyomi Analytics | Understand site usage and improve UX | ❌ None | Fully anonymous, GDPR/CCPA compliant |

**How our analytics works without cookies:**
- Uses a daily rotating hash of IP + User Agent
- Cannot track individuals or across sessions
- Data is aggregated and anonymized
- No consent banner required
- See our [Privacy Policy](/privacy#website-analytics) for details

---

## Cookies We DON'T Use

Unlike most websites, we don't use:

- ❌ **Advertising cookies** - We don't show ads or track you for ad targeting
- ❌ **Social media cookies** - No Facebook Pixel, Twitter tracking, etc.
- ❌ **Analytics cookies** — Our analytics is completely cookie-free
- ❌ **Preference cookies** - We store preferences in your account, not cookies
- ❌ **Third-party cookies** - We don't allow other companies to set cookies

---

## Third-Party Services

Some services we use may set cookies when you interact with them:

### Google OAuth

When you sign in with Google:
- Google may set cookies for authentication
- Google's Cookie Policy applies: [policies.google.com/technologies/cookies](https://policies.google.com/technologies/cookies)
- You're redirected to Google's domain, then back to Kyomi

### Stripe (Payment Processing)

When you enter payment information:
- Stripe may set cookies for fraud prevention
- Stripe's Cookie Policy applies: [stripe.com/cookies-policy/legal](https://stripe.com/cookies-policy/legal)
- Stripe uses hosted payment forms (not embedded)

**Important:** These services only set cookies when you explicitly interact with them (signing in, making payments).

---

## Managing Cookies

### Browser Controls

All modern browsers let you control cookies:

#### Block All Cookies
**Warning:** This will break Kyomi's login functionality.

- **Chrome** - Settings > Privacy > Cookies
- **Firefox** - Settings > Privacy > Cookies
- **Safari** - Preferences > Privacy > Cookies
- **Edge** - Settings > Cookies and site permissions

#### Block Third-Party Cookies
**Recommended:** This doesn't affect Kyomi since we don't use third-party cookies.

- Most browsers have this option in Privacy settings
- Often labeled "Block third-party cookies"

#### Clear Cookies

Deleting cookies will log you out of Kyomi:

- **Chrome** - Settings > Privacy > Clear browsing data
- **Firefox** - Settings > Privacy > Clear Data
- **Safari** - Preferences > Privacy > Manage Website Data
- **Edge** - Settings > Privacy > Clear browsing data

### Do Not Track (DNT)

We respect Do Not Track signals:

- If your browser sends DNT, we won't enable optional analytics
- DNT is set in your browser privacy settings
- Note: We don't use tracking cookies anyway

---

## Cookie Consent & GDPR

### Why No Cookie Banner?

Under GDPR and ePrivacy Directive:

- **Essential cookies** - Don't require consent (necessary for service operation)
- **Non-essential cookies** - Require explicit consent

Since we only use essential cookies, we don't need a cookie banner.

### Your Rights

Under GDPR, you have rights regarding cookies:

- **Right to information** - This policy explains our cookie use
- **Right to access** - See what cookies are set (use browser developer tools)
- **Right to deletion** - Clear cookies in your browser settings
- **Right to object** - We only use essential cookies, so objecting means you can't use Kyomi

---

## Changes to This Policy

We may update this Cookie Policy:

- **Notification** - We'll update the "Last Updated" date
- **Significant changes** - We'll notify you via email
- **Previous versions** - Available on request

---

## Questions About Cookies?

Contact us:

- **Email** - [privacy@kyomi.ai](mailto:privacy@kyomi.ai)
- **Privacy Policy** - [kyomi.ai/privacy](/privacy)

---

## Related Policies

- [Privacy Policy](/privacy) - How we handle your data
- [Terms of Service](/terms) - Terms for using Kyomi
- [Security](/security) - How we keep your data safe

---

*Last updated: February 21, 2026*
