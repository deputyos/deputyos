# Page rules / Bulk redirects

Cloudflare Bulk Redirects are preferred over Page Rules (deprecated UX).

## Lists

### `deputyos-dev-legacy` (priority 100)
Source URL: `*deputyos.dev/*`
Target URL: `https://www.deputyos.com/$2`
Status: 301
Preserve query string: yes
Subpath matching: yes

### `apex-to-www` (priority 200)
Source URL: `deputyos.com/*`
Target URL: `https://www.deputyos.com/$1`
Status: 301
Preserve query string: yes

### `try-shorthand` (priority 300)
Source URL: `try.deputyos.com/*`
Target URL: `https://www.deputyos.com/picker/`
Status: 302
Preserve query string: no
