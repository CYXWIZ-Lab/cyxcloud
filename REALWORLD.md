# CyxCloud: From Local to Production

This document outlines the strategy and steps to take CyxCloud from local development to a production system serving real users worldwide, with budget constraints in mind.

## Current State

### What We Have (Local)

| Component | Status | Location |
|-----------|--------|----------|
| Gateway | ✅ Working | Local Docker / Binary |
| Storage Nodes | ✅ Working | Local Docker / Binary |
| CLI | ✅ Working | GitHub Releases |
| Website (cyxwiz_web) | ✅ Working | localhost:3000 |
| Blockchain Programs | ✅ Deployed | Solana Devnet |
| PostgreSQL | ✅ Working | Local Docker |
| Redis | ✅ Working | Local Docker |

### What Users Need

1. **Download binaries** - CLI and Node software
2. **Connect to Gateway** - Central API endpoint
3. **Web Dashboard** - Manage files, subscriptions, nodes
4. **Blockchain** - Pay with CYXWIZ tokens (mainnet)

---

## Architecture: Distributed by Design

CyxCloud is designed to minimize central infrastructure costs:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        CyxCloud Production Architecture                      │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│   CENTRALIZED (We Host)              DISTRIBUTED (Community Hosts)          │
│   ─────────────────────              ──────────────────────────────         │
│                                                                              │
│   ┌─────────────────┐                ┌─────────────────────────────┐        │
│   │   Gateway API   │◀──────────────▶│     Storage Nodes (P2P)     │        │
│   │   (1 server)    │                │   (Community-run nodes)     │        │
│   └────────┬────────┘                │   - Node operators stake    │        │
│            │                         │   - Earn CYXWIZ rewards     │        │
│            │                         └─────────────────────────────┘        │
│   ┌────────┴────────┐                                                       │
│   │  Website/API    │                ┌─────────────────────────────┐        │
│   │  (Static + API) │                │     Solana Blockchain       │        │
│   └────────┬────────┘                │   - Payment processing      │        │
│            │                         │   - No hosting cost         │        │
│   ┌────────┴────────┐                └─────────────────────────────┘        │
│   │  PostgreSQL +   │                                                       │
│   │  Redis          │                                                       │
│   └─────────────────┘                                                       │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Key Insight:** Storage nodes are run by community members who earn tokens. We only need to host:
- 1 Gateway server
- 1 Website
- 1 PostgreSQL + Redis instance

---

## Budget-Friendly Hosting Options

### Tier 1: Free/Minimal ($0-20/month)

Best for: MVP launch, testing with early adopters

| Service | Provider | Cost | Purpose |
|---------|----------|------|---------|
| Gateway + DB | [Railway](https://railway.app) | Free tier / $5 | API + PostgreSQL + Redis |
| Website | [Vercel](https://vercel.com) | Free | Next.js frontend |
| Binaries | [GitHub Releases](https://github.com) | Free | CLI/Node downloads |
| Domain | [Cloudflare](https://cloudflare.com) | $10/year | cyxcloud.io |
| Blockchain | Solana | $0 | Mainnet (users pay fees) |

**Total: ~$5-15/month**

### Tier 2: Small Scale ($20-50/month)

Best for: 100-1000 users, stable service

| Service | Provider | Cost | Purpose |
|---------|----------|------|---------|
| Gateway | [Fly.io](https://fly.io) | $5-15 | 1 shared CPU, 256MB |
| PostgreSQL | [Neon](https://neon.tech) | Free | Serverless Postgres |
| Redis | [Upstash](https://upstash.com) | Free | Serverless Redis |
| Website | [Vercel](https://vercel.com) | Free | Next.js |
| CDN | [Cloudflare](https://cloudflare.com) | Free | Caching, DDoS protection |

**Total: ~$20-30/month**

### Tier 3: Production ($50-150/month)

Best for: 1000+ users, SLA requirements

| Service | Provider | Cost | Purpose |
|---------|----------|------|---------|
| Gateway | [DigitalOcean](https://digitalocean.com) | $24 | 2 vCPU, 2GB RAM |
| PostgreSQL | [DigitalOcean Managed](https://digitalocean.com) | $15 | 1GB, auto-backups |
| Redis | [DigitalOcean Managed](https://digitalocean.com) | $15 | 1GB |
| Website | [Vercel Pro](https://vercel.com) | $20 | Team features |
| Monitoring | [Grafana Cloud](https://grafana.com) | Free | 10k metrics |

**Total: ~$75-100/month**

---

## Recommended Approach: Start Free, Scale Up

### Phase 1: Free Launch (Week 1-2)

**Goal:** Get live with $0 hosting cost

1. **Website on Vercel (Free)**
   ```bash
   cd cyxwiz_web
   vercel deploy --prod
   ```

2. **Gateway on Railway (Free Tier)**
   ```bash
   # railway.app - connect GitHub repo
   # Set environment variables:
   DATABASE_URL=<railway-postgres>
   REDIS_URL=<railway-redis>
   SOLANA_RPC_URL=https://api.mainnet-beta.solana.com
   ```

3. **Binaries on GitHub Releases (Free)**
   - Already done: https://github.com/CYXWIZ-Lab/cyxcloud/releases

4. **Domain on Cloudflare ($10/year)**
   - Buy cyxcloud.io or similar
   - Point to Vercel (website) and Railway (API)

**Architecture:**
```
Users
  │
  ├─► cyxcloud.io (Vercel) ──► Website
  │
  ├─► api.cyxcloud.io (Railway) ──► Gateway API
  │
  └─► Download from GitHub ──► CLI/Node binaries
```

### Phase 2: Early Adopters (Week 3-4)

**Goal:** Onboard first node operators

1. **Deploy to Solana Mainnet**
   ```bash
   cd cyxwiz-blockchain
   anchor deploy --provider.cluster mainnet
   ```

2. **Create Founding Node Operator Program**
   - First 10 nodes get bonus CYXWIZ
   - Lower staking requirement (100 CYXWIZ instead of 500)
   - Community Discord/Telegram

3. **Monitor and Fix Issues**
   - Set up error tracking (Sentry free tier)
   - Basic metrics (Railway built-in)

### Phase 3: Public Beta (Month 2)

**Goal:** Open to all users

1. **Migrate to Fly.io or DigitalOcean** (if Railway limits hit)
2. **Add monitoring** (Grafana Cloud free tier)
3. **Launch marketing** (Twitter, Reddit, HackerNews)

---

## Step-by-Step Deployment Guide

### Step 1: Prepare Blockchain for Mainnet

```bash
# 1. Get mainnet SOL for deployment
solana config set --url https://api.mainnet-beta.solana.com
solana balance  # Need ~3 SOL for deployment

# 2. Deploy programs
cd cyxwiz-blockchain
anchor build
anchor deploy --provider.cluster mainnet

# 3. Update program IDs in code
# Edit: cyx_cloud/cyxcloud-gateway/src/blockchain/client.rs
# Edit: cyx_cloud/cyxcloud-node/src/blockchain/client.rs
```

### Step 2: Deploy Website to Vercel

```bash
# 1. Install Vercel CLI
npm i -g vercel

# 2. Login
vercel login

# 3. Deploy
cd cyxwiz_web
vercel --prod

# 4. Set environment variables in Vercel dashboard
NEXT_PUBLIC_API_URL=https://api.cyxcloud.io
NEXT_PUBLIC_SOLANA_RPC=https://api.mainnet-beta.solana.com
```

### Step 3: Deploy Gateway to Railway

1. Go to https://railway.app
2. New Project → Deploy from GitHub
3. Select `CYXWIZ-Lab/cyxcloud`
4. Set root directory: `/` (or create a railway.toml)
5. Add PostgreSQL plugin
6. Add Redis plugin
7. Set environment variables:

```env
DATABASE_URL=${{Postgres.DATABASE_URL}}
REDIS_URL=${{Redis.REDIS_URL}}
GATEWAY_HTTP_PORT=8080
GATEWAY_GRPC_PORT=50052
JWT_SECRET=<generate-secure-secret>
SOLANA_RPC_URL=https://api.mainnet-beta.solana.com
RUST_LOG=info
```

8. Deploy

### Step 4: Configure Domain

1. Buy domain on Cloudflare or Namecheap
2. Add to Cloudflare (free)
3. Create DNS records:
   ```
   A     cyxcloud.io      → Vercel IP
   CNAME api.cyxcloud.io  → railway-app-xxx.up.railway.app
   CNAME www              → cyxcloud.io
   ```

### Step 5: Update Binaries

Update default gateway URL in CLI:

```rust
// cyxcloud-cli/src/main.rs
#[clap(long, default_value = "https://api.cyxcloud.io")]
gateway: String,
```

Rebuild and create new release:
```bash
git tag v0.1.1
git push origin v0.1.1
# GitHub Actions will build and release
```

### Step 6: Create Node Operator Guide

Create `docs/NODE_OPERATOR_GUIDE.md`:
```markdown
# Become a CyxCloud Storage Node Operator

## Requirements
- Linux/macOS/Windows machine
- 100GB+ free storage
- Stable internet (10+ Mbps)
- 500 CYXWIZ tokens for staking

## Quick Start
1. Download: https://github.com/CYXWIZ-Lab/cyxcloud/releases
2. Configure node
3. Stake CYXWIZ
4. Start earning!
```

---

## Cost Projection

### Year 1 Costs

| Month | Users | Nodes | Hosting Cost | Revenue (est.) |
|-------|-------|-------|--------------|----------------|
| 1-3 | 10-50 | 3-5 | $0 (free tier) | $0 |
| 4-6 | 50-200 | 10-20 | $20/month | $100-500/month |
| 7-12 | 200-1000 | 20-50 | $50-100/month | $500-2000/month |

**Revenue Sources:**
- 10% platform fee on storage payments
- Premium plans (more storage, faster speeds)
- Enterprise contracts

### Break-Even Analysis

```
Monthly hosting: $50
Platform fee: 10%
Average user pays: 10 CYXWIZ/month ($5 at $0.50/CYXWIZ)
Platform revenue per user: $0.50/month

Break-even: 50 / 0.50 = 100 paying users
```

---

## Free Hosting Services Summary

| Service | Free Tier | Best For |
|---------|-----------|----------|
| **Vercel** | 100GB bandwidth, unlimited sites | Website hosting |
| **Railway** | $5 credit/month, 512MB RAM | Gateway + DB |
| **Neon** | 0.5GB storage, auto-suspend | PostgreSQL |
| **Upstash** | 10k requests/day | Redis |
| **Fly.io** | 3 shared VMs, 160GB bandwidth | Gateway (alternative) |
| **Render** | 750 hours/month | Gateway (alternative) |
| **PlanetScale** | 1B reads, 10M writes | MySQL (alternative) |
| **GitHub** | Unlimited public repos | Code + Releases |
| **Cloudflare** | Unlimited bandwidth | CDN + DNS |

---

## Checklist: Go Live

### Pre-Launch
- [ ] Deploy blockchain programs to mainnet
- [ ] Deploy website to Vercel
- [ ] Deploy gateway to Railway/Fly.io
- [ ] Configure domain and SSL
- [ ] Update CLI with production gateway URL
- [ ] Create v0.2.0 release with production URLs
- [ ] Test full flow: signup → payment → upload → download
- [ ] Set up error monitoring (Sentry)
- [ ] Create Discord/Telegram community

### Launch Day
- [ ] Announce on Twitter/X
- [ ] Post on Reddit (r/cryptocurrency, r/solana, r/selfhosted)
- [ ] Submit to Product Hunt
- [ ] Reach out to crypto influencers
- [ ] Monitor logs and metrics

### Post-Launch
- [ ] Respond to user feedback
- [ ] Fix critical bugs immediately
- [ ] Onboard first node operators
- [ ] Write blog post about architecture
- [ ] Create video tutorials

---

## Security Considerations

### Production Secrets

Never commit these to git:
```bash
# Generate secure secrets
openssl rand -base64 32  # JWT_SECRET
openssl rand -base64 32  # API keys
```

Store in:
- Railway: Environment variables (encrypted)
- Vercel: Environment variables (encrypted)
- Local: `.env.local` (gitignored)

### SSL/TLS

- Vercel: Automatic HTTPS
- Railway: Automatic HTTPS
- Cloudflare: Automatic HTTPS + DDoS protection

### Rate Limiting

Add to gateway (already implemented):
```rust
// 100 requests per minute per IP
.layer(RateLimitLayer::new(100, Duration::from_secs(60)))
```

---

## Monitoring (Free Options)

### Option 1: Grafana Cloud (Free Tier)

```yaml
# docker-compose.monitoring.yml
services:
  prometheus:
    image: prom/prometheus
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml

  grafana:
    image: grafana/grafana
    environment:
      GF_SECURITY_ADMIN_PASSWORD: admin
```

### Option 2: Railway Built-in

Railway provides:
- CPU/Memory graphs
- Request logs
- Error tracking

### Option 3: Better Uptime (Free)

- 50 monitors free
- Email/SMS alerts
- Status page

---

## Next Actions (Priority Order)

1. **Today:** Create accounts on Vercel, Railway, Cloudflare
2. **This Week:** Deploy website and gateway to free tiers
3. **Next Week:** Test full production flow
4. **Month 1:** Onboard 10 beta testers
5. **Month 2:** Public launch

---

## Questions to Decide

1. **Domain name:** cyxcloud.io? cyxwiz.cloud? other?
2. **Mainnet timing:** Deploy now or wait for more testing?
3. **Node operator incentives:** Founding operator bonus amount?
4. **Marketing budget:** Any budget for ads/influencers?

---

## Resources

- [Railway Docs](https://docs.railway.app)
- [Vercel Docs](https://vercel.com/docs)
- [Fly.io Docs](https://fly.io/docs)
- [Cloudflare Docs](https://developers.cloudflare.com)
- [Solana Mainnet Guide](https://docs.solana.com/clusters)

---

*This document should be updated as we progress through each phase.*
