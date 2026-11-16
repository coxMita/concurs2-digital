# Concurs: online multiple-choice exams

A web app for running timed multiple-choice exams online. Teachers manage
everything from the browser: admin accounts, exams, questions (with images),
student accounts, the live dashboard and Excel exports. The interface is in
Romanian.

- **Stack:** Rust (Axum 0.8, Tokio), PostgreSQL 16 (SQLx), Askama + HTMX, Caddy.
- **Capacity:** sized for 500 students taking an exam at the same time on a
  2 vCPU / 4 GB VPS (see [Load test](#load-test)).

---

## Deploying on a fresh Ubuntu VPS

### 1. Install Docker

```bash
curl -fsSL https://get.docker.com | sudo sh
sudo usermod -aG docker $USER   # log out and back in afterwards
```

### 2. Point your domain to the server (optional but recommended)

At your DNS provider, create an **A record** (for example `teste.liceul-meu.ro`)
pointing to the VPS public IP. Make sure ports **80** and **443** are open in
the VPS firewall:

```bash
sudo ufw allow 80,443/tcp && sudo ufw allow 443/udp   # if ufw is enabled
```

### 3. Get the code and start everything

```bash
git clone <repository-url> concurs && cd concurs
cp .env.example .env
docker compose up -d
```

That is all. The first start builds the image, which takes a few minutes.
Before the first start you can change these values in `.env` (the defaults
work as-is):

| Variable | Default | Meaning |
|---|---|---|
| `DOMAIN` | `localhost` | Your domain. With a real domain Caddy obtains a Let's Encrypt certificate automatically. With `localhost` it uses its own self-signed certificate (for testing on the server itself). |
| `SETUP_TOKEN` | *(empty)* | Protects `/setup`. If empty, a random token is generated and printed in the logs. |
| `POSTGRES_PASSWORD` | `concurs-change-me` | Database password. Postgres is not reachable from outside Docker, but **changing it before the first start is recommended**. |

The session/CSRF signing key is generated automatically on first start and
stored in the database.

### 4. Create the admin account (first run)

```bash
docker compose logs app
```

The log shows a block like this:

```
FIRST-RUN SETUP: no admin account exists yet.
Open:  https://teste.liceul-meu.ro/setup
Setup token (generated):  xxxxxxxxxxxxxxxxxxxxxxxx
```

Open the link, enter the token and choose the admin username and password.
After the first admin exists, `/setup` is permanently disabled (404). You can
add more admins from **Admin → Cont**.

### 5. Use it

- Admin panel: `https://<domain>/admin`
- Students: `https://<domain>/` (username + password only)

A typical exam:

1. **Teste → + Test nou**: set the title, duration and what students see after
   submitting.
2. Add questions one by one, or paste many at once with **Import în masă**.
3. **Elevi → Generează conturi**: paste names and/or enter a number. Download
   the Excel file or print the slips. **Passwords are shown only once.**
4. **Setări → Stare: Deschis** when the class is ready. Students waiting on the
   "Testul nu a început încă" screen are moved into the exam automatically.
5. Watch **Monitorizare**. Use **Rezultate → Descarcă** for the Excel file.
6. **Stare: Închis** finalizes every attempt still in progress.

## Updating

```bash
cd concurs
git pull
docker compose up -d --build
```

Database migrations run automatically when the app starts. Data, uploaded
images and certificates live in named Docker volumes and survive updates.

## Backups

All data (exams, accounts, answers, sessions) is in PostgreSQL. Uploaded
question images are in the `uploads` volume.

Manual database dump:

```bash
docker compose exec -T db pg_dump -U concurs -Fc concurs > backup-$(date +%F).dump
```

Daily backup at 02:30 with 14 days kept (`crontab -e` as the user that runs Docker):

```cron
30 2 * * * cd /home/ubuntu/concurs && mkdir -p backups && docker compose exec -T db pg_dump -U concurs -Fc concurs > backups/db-$(date +\%F).dump && find backups -name 'db-*.dump' -mtime +14 -delete
```

Back up the images too:

```bash
docker run --rm -v concurs_uploads:/u -v "$PWD/backups":/b alpine tar czf /b/uploads-$(date +%F).tgz -C /u .
```

(The volume name is `<folder>_uploads`; check it with `docker volume ls`.)

Restore:

```bash
docker compose exec -T db pg_restore -U concurs -d concurs --clean --if-exists < backup.dump
```

Copy backups off the server, for example with `rsync` or `rclone`.

## Local development

Requirements: Rust (stable, edition 2024) and Docker.

```bash
docker compose -f docker-compose.dev.yml up -d   # Postgres on 127.0.0.1:5432
cat > .env <<'EOF'
DATABASE_URL=postgres://concurs:concurs@localhost:5432/concurs
BIND_ADDR=127.0.0.1:8080
COOKIE_SECURE=false
DOMAIN=localhost:8080
EOF
cargo run
```

Open <http://localhost:8080/setup>. The setup token is printed in the terminal.

### Environment variables

| Variable | Default | |
|---|---|---|
| `DATABASE_URL` | `postgres://concurs:concurs@localhost:5432/concurs` | |
| `BIND_ADDR` | `0.0.0.0:8080` | |
| `DOMAIN` | `localhost` | Used in log messages and to decide whether to send HSTS. |
| `SETUP_TOKEN` | *(generated)* | |
| `COOKIE_SECURE` | `true` | Set to `false` for plain-HTTP local development. |
| `DB_MAX_CONNECTIONS` | `25` | |
| `UPLOAD_DIR` | `./data/uploads` | |
| `STATIC_DIR` | `./static` | |
| `MAX_UPLOAD_MB` | `3` | |
| `ARGON2_MEMORY_KIB` / `ARGON2_ITERATIONS` | `19456` / `2` | About 50–100 ms per verification on a small VPS. |
| `LOGIN_FAIL_PER_USER_PER_MIN` | `10` | Failed attempts per username per minute. |
| `LOGIN_PER_IP_PER_MIN` | `600` | Login requests per IP per minute. Generous, because a whole school shares one IP. |
| `FINALIZER_INTERVAL_SECS` | `30` | How often expired attempts are finalized. |
| `RUST_LOG` | `info` | |

### Tests and checks

```bash
cargo test                       # unit + integration tests (needs the dev Postgres)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The integration tests (`tests/app.rs`) use `#[sqlx::test]`, which creates a
temporary database per test.

SQL queries are checked at compile time. After changing a query, refresh the
offline data used by Docker builds and commit it:

```bash
cargo install sqlx-cli --no-default-features --features postgres,rustls,sqlx-toml
cargo sqlx prepare -- --all-targets
```

## Load test

`loadtest/k6.js` simulates students who log in, open the exam, answer every
question with realistic pauses and submit. It runs through Docker, so k6 does
not need to be installed:

```bash
./loadtest/run.sh            # see the script for options (default: 500 students)
```

The script creates a test exam with 500 accounts, runs k6 against it and
prints p95 latencies and the error rate. Delete the "Test de încărcare" exam
afterwards.

Measured on the full Docker stack (Caddy + app + Postgres) pinned to **2 CPU
cores and 4 GB in total**. 500 students, 20 questions, all logins within one
minute, 3–8 s per question, some answers changed, the teacher's dashboard
polled every 5 s:

| Request | p95 | p99 |
|---|---|---|
| login (Argon2 verify) | 104 ms | 160 ms |
| exam page | 23 ms | 36 ms |
| autosave | 19 ms | 27 ms |
| submit | 24 ms | 35 ms |
| live dashboard | 27 ms | 29 ms |

25,016 requests, 0 errors, 500/500 exams submitted. Peak app memory was
about 45 MB.

## Project layout

```
src/
  main.rs, app.rs (router + middleware), config.rs, error.rs, state.rs, web.rs
  auth/      sessions, CSRF, login rate limiting, extractors
  routes/    setup, auth, student, admin/{account,exams,questions,students,results}
  db/        queries per aggregate (sqlx::query! macros)
  services/  grading, shuffle, credentials, export (Excel), uploads, import_format,
             exam_cache, finalizer, hashing
templates/   Askama templates (Romanian UI)
static/      app.css (design tokens → base → components → pages), JS, Inter font
migrations/  SQL migrations (run automatically at startup)
tests/       integration tests
loadtest/    k6 load test
```

### Question import format

```
Î: Care este capitala României?
P: 1
T: simplu
+ București
- Cluj-Napoca

Î: Care dintre numerele următoare sunt prime?
T: multiplu
+ 2
+ 3
- 4
```

- `Î:` starts a question; the lines after it continue its text.
- `P:` sets the points (default 1).
- `T:` is `simplu` or `multiplu`. If omitted, it is inferred from the number of
  correct options.
- `+` marks a correct option and `-` a wrong one (2–8 options per question).
- Lines starting with `#` are comments.
- A text line starting with `\` is taken literally.

Export produces the same format, so a question set can be reused in another
exam.

## Security notes

- Time is enforced on the server. Answers after the deadline are rejected in
  SQL. Expired attempts are finalized lazily and by a background task every
  30 s.
- Correct answers are never sent to the student's browser before they submit.
  After submitting, they are sent only if the exam shows answers.
- One active session per student: a new login invalidates the older sessions.
  Re-logins during an exam are flagged on the dashboard.
- Forms and HTMX requests are protected by CSRF tokens. Cookies are HttpOnly,
  Secure and SameSite=Lax. The CSP forbids inline scripts.
- Passwords are hashed with Argon2id, on a bounded blocking pool. Failed logins
  are rate-limited per username, and there is a generous limit per IP.
- Uploaded images are checked by their content (magic bytes), size-limited and
  stored under random file names.
