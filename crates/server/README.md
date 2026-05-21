# Wisp Server Deployment with Nginx Proxy Manager (NPM)

This directory contains the necessary files to deploy the `wisp-server` alongside Nginx Proxy Manager for easy SSL management and reverse proxying.

## Prerequisites
- Docker and Docker Compose installed.

## Getting Started

1. **Start the containers:**
   From this directory, run:
   ```bash
   docker-compose up -d
   ```

2. **Access Nginx Proxy Manager Admin:**
   Open your browser and go to `http://localhost:81`.
   - **Default Email:** `admin@example.com`
   - **Default Password:** `changeme`
   *(You will be prompted to change these on first login)*

3. **Configure Proxy Host:**
   - Go to **Hosts** -> **Proxy Hosts** -> **Add Proxy Host**.
   - **Domain Names:** Your domain (e.g., `wisp.yourdomain.com`).
   - **Scheme:** `http`
   - **Forward Hostname / IP:** `wisp-server`
   - **Forward Port:** `8787`
   - **SSL:** Go to the SSL tab and select "Request a new SSL Certificate" to enable HTTPS via Let's Encrypt.

4. **Verify Deployment:**
   Visit `https://wisp.yourdomain.com/healthz` (replace with your domain). You should see `ok`.

## File Structure
- `Dockerfile` (at project root): Multi-stage build for the Rust server.
- `docker-compose.yml`: Defines the `wisp-server` and `npm` services.
- `npm/`: Local directory where NPM data and SSL certificates are persisted.
