"""The secret-shape corpus for ferb-sanitize (rust/crates/ferb-sanitize/tests/shapes.rs). NOT recorded from Python:
this corpus measures which secret shapes the Rust redactor masks and which ordinary strings it leaves alone; the
Python build is not run.

Run from the repo root:  python3 rust/fixtures/sanitize-shapes/make.py

Every value is invented here from a seeded random source (the same run writes the same file). None is a real
credential: random bodies fail every vendor's checksum (GitHub's CRC32, npm's, PyPI's macaroon signature), and the
fixed parts are the public prefixes the vendors document. The values are written as they are (no encoding): they are
key-shaped on purpose, and the repo's commit gate needs an allow entry for this file (see README.md).

Sources (which public scanner list each shape comes from):
  gitleaks:<rule>   gitleaks' default config (config/gitleaks.toml), by rule id
  trufflehog:<det>  trufflehog's detectors (pkg/detectors/<det>)
  github            GitHub secret scanning's supported patterns ("Supported secret scanning patterns")
  detect-secrets:<plugin>  Yelp detect-secrets' plugins
  ferb              a shape FERB already masks for its own reasons (cards, codes, labels)
"""

import json
import random
import string
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
RNG = random.Random(20261115)

UP = string.ascii_uppercase
LO = string.ascii_lowercase
DG = string.digits
HEX = "0123456789abcdef"
AN = UP + LO + DG
B64URL = AN + "-_"
B64 = AN + "+/"
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def r(chars: str, n: int) -> str:
    """n random characters from `chars`, with at least one digit and one letter when the set has both (a vendor's
    random body nearly always has both; an all-letter draw would be a different test)."""
    while True:
        out = "".join(RNG.choice(chars) for _ in range(n))
        has_d = any(c.isdigit() for c in chars)
        has_a = any(c.isalpha() for c in chars)
        if n < 6 or ((not has_d or any(c.isdigit() for c in out)) and (not has_a or any(c.isalpha() for c in out))):
            return out


def uuid() -> str:
    return "-".join(r(HEX, n) for n in (8, 4, 4, 4, 12))


def pem_body(lines: int = 4) -> str:
    return "\n".join(r(B64, 64) for _ in range(lines))


POS: list[dict] = []
NEG: list[dict] = []
IDENT: list[dict] = []


def P(id_: str, shape: str, sources: list[str], text: str, secrets: list[str], limit: str = "") -> None:
    """A secret shape: every string in `secrets` must be gone from the redacted text. `limit`: a declared gap (the
    test asserts it still leaks, so the declared list stays true)."""
    for s in secrets:
        assert s in text, (id_, s)
    POS.append({"id": id_, "shape": shape, "sources": sources, "text": text, "secrets": secrets,
                **({"limit": limit} if limit else {})})


def N(id_: str, what: str, text: str, known: str = "") -> None:
    """Ordinary text: must come back unchanged. `known`: a false positive of a rule ported from Python, kept as it is
    (the test asserts it is still masked, so the list stays true; changing a Python rule is not this layer's)."""
    NEG.append({"id": id_, "what": what, "text": text, **({"known_fp": known} if known else {})})


def I(id_: str, what: str, text: str, values: list[str]) -> None:
    """A personal identifier: reported, never asserted (masking identifiers is a policy question for the owner)."""
    IDENT.append({"id": id_, "what": what, "text": text, "values": values})


# ---------------------------------------------------------------- cloud

v = "AKIA" + r(UP + DG, 16)
P("aws-akid", "AWS access key id (AKIA)", ["gitleaks:aws-access-token", "github", "trufflehog:aws",
  "detect-secrets:AWSKeyDetector"], f"aws_access_key_id = {v}", [v])
v = "ASIA" + r(UP + DG, 16)
P("aws-asia", "AWS temporary key id (ASIA)", ["gitleaks:aws-access-token", "github"],
  f"Assumed role, credentials {v} expire at 14:00", [v])
s40 = r(B64, 40)
P("aws-secret-labelled", "AWS secret access key, labelled", ["gitleaks:aws-access-token", "github", "trufflehog:aws"],
  f"aws_secret_access_key = {s40}", [s40])
s40 = r(B64, 40)
P("aws-secret-export", "AWS secret access key, export line", ["github", "trufflehog:aws"],
  f'export AWS_SECRET_ACCESS_KEY="{s40}"', [s40])
k, s40 = "AKIA" + r(UP + DG, 16), r(B64, 40)
P("aws-secret-csv", "AWS key pair as the console's CSV (secret unlabelled on its line)", ["github", "trufflehog:aws"],
  f"User name,Access key ID,Secret access key\ndeploy-bot,{k},{s40}", [k, s40])
v = r(B64, 300) + "=="
P("aws-session-token", "AWS session token, labelled", ["github"], f"AWS_SESSION_TOKEN={v}", [v])
kid, body = r(HEX, 40), pem_body(6).replace("\n", "\\n")
P("gcp-service-account", "GCP service-account JSON private_key", ["gitleaks:private-key", "github",
  "trufflehog:gcp"],
  '{"type": "service_account", "project_id": "demo-proj-4821", "private_key_id": "' + kid + '", '
  '"private_key": "-----BEGIN PRIVATE KEY-----\\n' + body + '\\n-----END PRIVATE KEY-----\\n", '
  '"client_email": "bot@demo-proj-4821.iam.gserviceaccount.com"}', [body[:64], body[-64:]])
v = "AIza" + r(B64URL, 35)
P("gcp-api-key", "Google API key (AIza)", ["gitleaks:gcp-api-key", "github", "trufflehog:googleapikey"],
  f'const firebaseConfig = {{ apiKey: "{v}", projectId: "demo" }};', [v])
v = r(B64, 86) + "=="
P("azure-storage-conn", "Azure storage account key in a connection string", ["github", "trufflehog:azurestorage"],
  f"DefaultEndpointsProtocol=https;AccountName=demostore;AccountKey={v};EndpointSuffix=core.windows.net", [v])
sig = r(AN, 20) + "%2B" + r(AN, 20) + "%3D"
P("azure-sas-url", "Azure SAS URL sig=", ["github", "trufflehog:azuresastoken"],
  f"https://demostore.blob.core.windows.net/c/f.txt?sv=2022-11-02&ss=b&srt=o&sp=r&se=2026-12-01T00:00:00Z"
  f"&spr=https&sig={sig}", [sig])
sig = r(AN, 20) + "%2F" + r(AN, 21) + "%3D"
P("azure-sas-conn", "Azure SAS in a connection string (SharedAccessSignature=…sig=)", ["github"],
  f"BlobEndpoint=https://demostore.blob.core.windows.net/;SharedAccessSignature=sv=2022-11-02&ss=b&srt=sco&sp=rl"
  f"&se=2026-12-01T00:00:00Z&sig={sig}", [sig])
v = r(AN, 3) + r(DG, 1) + "Q~" + r(AN + "_~.-", 34)
P("azure-ad-secret-bare", "Azure AD client secret (…Q~…), unlabelled", ["gitleaks:azure-ad-client-secret", "github"],
  f"the app registration shows {v} once, copy it now", [v])
v = r(AN, 3) + r(DG, 1) + "Q~" + r(AN + "_~.-", 34)
P("azure-ad-secret-labelled", "Azure AD client secret, labelled", ["gitleaks:azure-ad-client-secret", "github"],
  f"AZURE_CLIENT_SECRET={v}", [v])
v = "dop_v1_" + r(HEX, 64)
P("digitalocean-pat", "DigitalOcean token (dop_v1_)", ["gitleaks:digitalocean-pat", "github",
  "trufflehog:digitaloceanv2"], f"doctl auth init -t {v}", [v])
v = "doo_v1_" + r(HEX, 64)
P("digitalocean-oauth", "DigitalOcean OAuth token (doo_v1_)", ["gitleaks:digitalocean-access-token", "github"],
  f"token: {v}", [v])
v = r(B64URL, 40)
P("cloudflare-token-labelled", "Cloudflare API token, labelled", ["gitleaks:cloudflare-api-key", "trufflehog:cloudflareapitoken"],
  f"CLOUDFLARE_API_TOKEN={v}", [v])
v = r(HEX, 37)
P("cloudflare-global-key", "Cloudflare global API key in X-Auth-Key", ["gitleaks:cloudflare-global-api-key",
  "trufflehog:cloudflareglobalapikey"], f"X-Auth-Key: {v}", [v])
v = "HRKU-AA" + r(B64URL, 58)
P("heroku-v2", "Heroku API key (HRKU-AA)", ["gitleaks:heroku-api-key-v2", "github"],
  f"heroku authorizations:create printed {v}", [v])
v = uuid()
P("heroku-legacy-labelled", "Heroku API key (UUID), labelled", ["gitleaks:heroku-api-key", "trufflehog:heroku"],
  f"HEROKU_API_KEY={v}", [v])
v = r(HEX, 64)
P("linode-labelled", "Linode token, labelled", ["trufflehog:linode", "github"], f"LINODE_CLI_TOKEN={v}", [v])
v = "LTAI" + r(LO + UP + DG, 20)
P("alibaba-akid", "Alibaba Cloud AccessKey id (LTAI)", ["gitleaks:alibaba-access-key-id", "github"],
  f"accessKeyId: {v}", [v])
v = r(AN, 30)
P("alibaba-secret-labelled", "Alibaba Cloud AccessKey secret, labelled", ["gitleaks:alibaba-secret-key", "github"],
  f"accessKeySecret: {v}", [v])

# ---------------------------------------------------------------- source hosts and registries

for p in ("ghp", "gho", "ghu", "ghs", "ghr"):
    v = f"{p}_" + r(AN, 36)
    P(f"github-{p}", f"GitHub token ({p}_)", ["gitleaks:github-pat", "github", "trufflehog:github"],
      f"the CLI printed {v} and exited", [v])
v = "github_pat_11" + r(UP + DG, 20) + "_" + r(AN, 59)
P("github-fine-grained", "GitHub fine-grained token (github_pat_)", ["gitleaks:github-fine-grained-pat", "github"],
  f"token = {v}", [v])
v = "ghp_" + r(AN, 36)
P("github-in-clone-url", "GitHub token as a clone URL's userinfo", ["gitleaks:github-pat", "github"],
  f"git clone https://{v}@github.com/acme/app.git", [v])
for p, n in (("glpat-", 20), ("gldt-", 20), ("glrt-", 20), ("glptt-", 40), ("glcbt-", 20), ("glsoat-", 20),
             ("glft-", 20), ("gloas-", 64), ("glimt-", 25), ("glagent-", 50)):
    v = p + (r(HEX, n) if p == "glptt-" else r(B64URL, n))
    P(f"gitlab-{p[:-1]}", f"GitLab token ({p})", [f"gitleaks:gitlab-{p[:-1]}", "github", "trufflehog:gitlab"],
      f"CI variable set to {v}", [v])
v = "GR1348941" + r(B64URL, 20)
P("gitlab-runner-registration", "GitLab runner registration token (GR1348941)", ["gitleaks:gitlab-rrt", "github"],
  f"gitlab-runner register --registration-token {v}", [v])
v = "ATBB" + r(AN, 32)
P("bitbucket-app-password", "Bitbucket app password (ATBB)", ["github", "trufflehog:bitbucketapppassword"],
  f"bitbucket {v}", [v])
v = "ATBB" + r(AN, 32)
P("bitbucket-app-password-bare", "Bitbucket app password (ATBB), unlabelled", ["github"],
  f"curl -u deploy:{v} https-less call failed", [v])
v = "ATATT3xFfGF0" + r(B64URL, 170) + "=" + r(HEX, 8).upper()
P("atlassian-api-token", "Atlassian API token (ATATT)", ["gitleaks:atlassian-api-token", "github"],
  f"jira {v}", [v])
v = "npm_" + r(AN, 36)
P("npm-token", "npm token (npm_)", ["gitleaks:npm-access-token", "github", "trufflehog:npmtoken"],
  f"//registry.npmjs.org/:_authToken={v}", [v])
v = r(B64, 24) + "="
P("npmrc-auth", ".npmrc _auth (base64 user:pass)", ["github"], f"_auth={v}", [v])
v = "pypi-AgEIcHlwaS5vcmc" + r(B64URL, 70)
P("pypi-token", "PyPI upload token (pypi-AgEI…)", ["gitleaks:pypi-upload-token", "github", "trufflehog:pypi"],
  f"[pypi]\nusername = __token__\npassword = {v}", [v])
v = "pypi-AgEIcHlwaS5vcmc" + r(B64URL, 70)
P("pypi-token-bare", "PyPI upload token, unlabelled", ["gitleaks:pypi-upload-token", "github"],
  f"twine upload -u __token__ -p {v} dist/*", [v])
v = "rubygems_" + r(HEX, 48)
P("rubygems", "RubyGems key (rubygems_)", ["gitleaks:rubygems-api-token", "github"],
  f":rubygems_api_key: {v}", [v])
v = "rubygems_" + r(HEX, 48)
P("rubygems-bare", "RubyGems key, unlabelled", ["gitleaks:rubygems-api-token", "github"],
  f"gem push with {v}", [v])
v = "dckr_pat_" + r(B64URL, 27)
P("dockerhub-pat", "Docker Hub token (dckr_pat_)", ["github", "trufflehog:dockerhub"],
  f"docker login -u acme -p {v}", [v])
v = "dckr_pat_" + r(B64URL, 27)
P("dockerhub-pat-bare", "Docker Hub token, unlabelled", ["github"], f"the CI log shows {v}", [v])
v = "cio" + r(AN, 32)
P("cratesio", "crates.io token (cio)", ["github"], f"cargo login {v}", [v])

# ---------------------------------------------------------------- AI providers

v = "sk-" + r(AN, 48)
P("openai-legacy", "OpenAI key (sk-)", ["gitleaks:openai-api-key", "github", "trufflehog:openai"], f"openai {v}", [v])
v = "sk-proj-" + r(B64URL, 156)
P("openai-project", "OpenAI project key (sk-proj-)", ["gitleaks:openai-api-key", "github"], f"OPENAI_API_KEY={v}", [v])
v = "sk-svcacct-" + r(B64URL, 120)
P("openai-svcacct", "OpenAI service-account key (sk-svcacct-)", ["gitleaks:openai-api-key", "github"],
  f"openai {v}", [v])
v = "sk-ant-api03-" + r(B64URL, 93) + "AA"
P("anthropic", "Anthropic key (sk-ant-api03-)", ["gitleaks:anthropic-api-key", "github", "trufflehog:anthropic"],
  f"x-api-key: {v}", [v])
v = "sk-ant-admin01-" + r(B64URL, 93) + "AA"
P("anthropic-admin", "Anthropic admin key (sk-ant-admin01-)", ["gitleaks:anthropic-admin-api-key", "github"],
  f"anthropic {v}", [v])
v = "hf_" + r(AN, 34)
P("huggingface", "Hugging Face token (hf_)", ["gitleaks:huggingface-access-token", "github", "trufflehog:huggingface"],
  f"hf {v}", [v])
v = "api_org_" + r(UP + LO, 34)
P("huggingface-org", "Hugging Face org token (api_org_)", ["gitleaks:huggingface-organization-api-token", "github"],
  f"org {v}", [v])
v = "r8_" + r(AN, 37)
P("replicate", "Replicate token (r8_)", ["github", "trufflehog:replicate"], f"REPLICATE_API_TOKEN={v}", [v])
v = "r8_" + r(AN, 37)
P("replicate-bare", "Replicate token, unlabelled", ["github"], f"replicate run with {v} now", [v])
v = "gsk_" + r(AN, 52)
P("groq", "Groq key (gsk_)", ["github", "trufflehog:groq"], f"groq {v}", [v])
v = r(AN, 40)
P("cohere-labelled", "Cohere key, labelled", ["trufflehog:cohere"], f"COHERE_API_KEY={v}", [v])
v = r(AN, 32)
P("mistral-labelled", "Mistral key, labelled", ["trufflehog:mistral"], f'MISTRAL_API_KEY="{v}"', [v])
v = "xai-" + r(AN, 80)
P("xai", "xAI key (xai-)", ["github", "trufflehog:xai"], f"xai {v}", [v])
v = "sk-or-v1-" + r(HEX, 64)
P("openrouter", "OpenRouter key (sk-or-v1-)", ["github", "trufflehog:openrouter"], f"openrouter {v}", [v])
v = "pplx-" + r(AN, 48)
P("perplexity", "Perplexity key (pplx-)", ["github", "trufflehog:perplexity"], f"use {v} for search", [v])
v = "lsv2_pt_" + r(HEX, 32) + "_" + r(HEX, 10)
P("langsmith", "LangSmith key (lsv2_pt_)", ["github", "trufflehog:langsmith"], f"tracing with {v}", [v])
v = "pcsk_" + r(AN, 6) + "_" + r(AN, 60)
P("pinecone", "Pinecone key (pcsk_)", ["github"], f"pinecone {v}", [v])

# ---------------------------------------------------------------- payments

v = "sk_live_" + r(AN, 99)
P("stripe-secret", "Stripe secret key (sk_live_)", ["gitleaks:stripe-access-token", "github", "trufflehog:stripe"],
  f"Stripe.api_key = '{v}'", [v])
v = "rk_live_" + r(AN, 99)
P("stripe-restricted", "Stripe restricted key (rk_live_)", ["gitleaks:stripe-access-token", "github"],
  f"stripe {v}", [v])
v = "sk_test_" + r(AN, 99)
P("stripe-test", "Stripe test secret key (sk_test_)", ["gitleaks:stripe-access-token", "github"], f"stripe {v}", [v])
v = "whsec_" + r(AN, 32)
P("stripe-webhook", "Stripe webhook signing secret (whsec_)", ["github"], f"stripe listen printed {v}", [v])
v = r(AN, 24)
P("razorpay-secret-labelled", "Razorpay key secret, labelled", ["trufflehog:razorpay"],
  f"RAZORPAY_KEY_SECRET={v}", [v])
v = "access_token$production$" + r(LO + DG, 16) + "$" + r(HEX, 32)
P("braintree", "Braintree/PayPal access token (access_token$production$…)", ["github", "trufflehog:braintreepayments"],
  f"gateway = Braintree::Gateway.new(access_token: \"{v}\")", [v])
v = "A21AA" + r(B64URL, 92)
P("paypal-access-token", "PayPal OAuth access token (A21AA…)", ["github"], f"paypal {v}", [v])
v = r(B64URL, 80)
P("paypal-secret-labelled", "PayPal client secret, labelled", ["trufflehog:paypaloauth"],
  f"PAYPAL_CLIENT_SECRET={v}", [v])
v = "sq0atp-" + r(B64URL, 22)
P("square-atp", "Square access token (sq0atp-)", ["gitleaks:square-access-token", "github", "trufflehog:square"],
  f"square {v}", [v])
v = "sq0csp-" + r(B64URL, 43)
P("square-csp", "Square OAuth secret (sq0csp-)", ["gitleaks:square-secret", "github"], f"square {v}", [v])
v = "EAAA" + r(B64URL, 60)
P("square-eaaa", "Square access token (EAAA)", ["gitleaks:square-access-token", "github"], f"square {v}", [v])
for p in ("shpat_", "shpss_", "shpca_", "shppa_"):
    v = p + r(HEX, 32)
    P(f"shopify-{p[:-1]}", f"Shopify token ({p})", [f"gitleaks:shopify-{'access-token' if p == 'shpat_' else 'shared-secret' if p == 'shpss_' else 'custom-access-token' if p == 'shpca_' else 'private-app-access-token'}", "github"],
      f"X-Shopify-Access: {v}", [v])

# ---------------------------------------------------------------- messaging

v = "xoxb-" + r(DG, 12) + "-" + r(DG, 13) + "-" + r(AN, 24)
P("slack-bot", "Slack bot token (xoxb-)", ["gitleaks:slack-bot-token", "github", "trufflehog:slack"], f"bot {v}", [v])
v = "xoxp-" + r(DG, 12) + "-" + r(DG, 12) + "-" + r(DG, 13) + "-" + r(HEX, 32)
P("slack-user", "Slack user token (xoxp-)", ["gitleaks:slack-user-token", "github"], f"user {v}", [v])
v = "xapp-1-A0" + r(UP + DG, 9) + "-" + r(DG, 13) + "-" + r(HEX, 64)
P("slack-app", "Slack app-level token (xapp-)", ["gitleaks:slack-app-token", "github"], f"SLACK_APP_TOKEN={v}", [v])
v = "xapp-1-A0" + r(UP + DG, 9) + "-" + r(DG, 13) + "-" + r(HEX, 64)
P("slack-app-bare", "Slack app-level token, unlabelled", ["gitleaks:slack-app-token", "github"],
  f"socket mode uses {v}", [v])
v = "xoxe.xoxp-1-" + r(AN, 160)
P("slack-config-token", "Slack configuration token (xoxe.xoxp-)", ["gitleaks:slack-config-access-token", "github"],
  f"slack {v}", [v])
v = "xoxe-1-" + r(AN, 140)
P("slack-refresh-token", "Slack refresh token (xoxe-)", ["gitleaks:slack-config-refresh-token", "github"],
  f"slack {v}", [v])
t, b, w = "T0" + r(UP + DG, 8), "B0" + r(UP + DG, 8), r(AN, 24)
P("slack-webhook", "Slack incoming webhook URL", ["gitleaks:slack-webhook-url", "github", "trufflehog:slackwebhook"],
  f"curl -X POST https://hooks.slack.com/services/{t}/{b}/{w} -d @msg.json", [w])
v = "M" + r(B64URL, 25) + "." + r(B64URL, 6) + "." + r(B64URL, 38)
P("discord-bot", "Discord bot token", ["gitleaks:discord-api-token", "github", "trufflehog:discordbottoken"],
  f"client.login('{v}')", [v])
i, w = r(DG, 18), r(B64URL, 68)
P("discord-webhook", "Discord webhook URL", ["gitleaks:discord-webhook-url", "trufflehog:discordwebhook"],
  f"https://discord.com/api/webhooks/{i}/{w}", [w])
i, w = r(DG, 10), "AA" + r(B64URL, 33)
P("telegram-bot", "Telegram bot token (123456789:AA…)", ["gitleaks:telegram-bot-api-token", "github",
  "trufflehog:telegrambottoken"], f"bot = telebot.TeleBot(\"{i}:{w}\")", [w])
v = "SK" + r(HEX, 32)
P("twilio-sk", "Twilio API key (SK…)", ["gitleaks:twilio-api-key", "github", "trufflehog:twilio"], f"twilio {v}", [v])
a, t = "AC" + r(HEX, 32), r(HEX, 32)
P("twilio-pair", "Twilio account SID + auth token (AC…:…)", ["github", "trufflehog:twilio"], f"{a}:{t}", [t])
t = r(HEX, 32)
P("twilio-auth-labelled", "Twilio auth token, labelled", ["github", "trufflehog:twilio"], f"TWILIO_AUTH_TOKEN={t}", [t])
v = "SG." + r(B64URL, 22) + "." + r(B64URL, 43)
P("sendgrid", "SendGrid key (SG.…)", ["gitleaks:sendgrid-api-token", "github", "trufflehog:sendgrid"],
  f"sendgrid {v}", [v])
v = "key-" + r(HEX, 32)
P("mailgun", "Mailgun private key (key-…)", ["gitleaks:mailgun-private-api-token", "github", "trufflehog:mailgun"],
  f"auth=('api', '{v}')", [v])
v = r(HEX, 32) + "-us12"
P("mailchimp", "Mailchimp key (…-us12)", ["gitleaks:mailchimp-api-key", "github", "trufflehog:mailchimp"],
  f"mailchimp client {v}", [v])
v = uuid()
P("postmark-labelled", "Postmark server token, labelled", ["trufflehog:postmark", "github"],
  f"POSTMARK_SERVER_TOKEN={v}", [v])

# ---------------------------------------------------------------- databases and services

pw = r(AN + "!#%", 14)
P("postgres-url", "postgres:// URL password", ["gitleaks:generic (url)", "trufflehog:uri", "github"],
  f"DATABASE_URL=postgres://app:{pw}@db.internal:5432/app", [pw])
pw = r(AN, 12)
P("mysql-url", "mysql:// URL password", ["trufflehog:uri"], f"mysql://root:{pw}@127.0.0.1:3306/shop", [pw])
pw = r(AN, 16)
P("mongodb-srv", "mongodb+srv:// URL password", ["github", "trufflehog:mongodb"],
  f'client = MongoClient("mongodb+srv://admin:{pw}@cluster0.ab1cd.mongodb.net/?retryWrites=true")', [pw])
pw = r(AN, 16)
P("redis-url", "redis:// URL password (empty user)", ["trufflehog:redis"], f"REDIS_URL=redis://:{pw}@cache:6379/0", [pw])
pw = r(AN, 16)
P("amqp-url", "amqp:// URL password", ["trufflehog:uri"], f"amqp://guest:{pw}@mq.internal:5672/vhost", [pw])
pw = r(AN, 12)
P("jdbc-query", "JDBC URL ?password=", ["github", "trufflehog:jdbc"],
  f"jdbc:postgresql://db:5432/app?user=app&password={pw}&ssl=true", [pw])
pw = r(AN, 12) + "!"
P("jdbc-sqlserver", "JDBC SQL Server ;password=", ["trufflehog:jdbc"],
  f"jdbc:sqlserver://db.internal:1433;databaseName=shop;user=sa;password={pw};encrypt=true", [pw])
pw = r(AN, 12)
P("odbc-pwd", "ODBC connection string Pwd=", ["trufflehog:sqlserver"],
  f"Driver={{ODBC Driver 18 for SQL Server}};Server=tcp:db,1433;Uid=sa;Pwd={pw};Encrypt=yes;", [pw])
pw = r(AN, 12)
P("adonet-password", "ADO.NET connection string Password=", ["trufflehog:sqlserver"],
  f'"Server=db;Database=shop;User Id=app;Password={pw};"', [pw])
v = ("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJvbGUiOiJzZXJ2aWNlX3JvbGUiLCJpYXQiOjE3MDAwMDAwMDB9."
     + r(B64URL, 43))
P("supabase-service-role", "Supabase service_role key (a JWT)", ["github", "trufflehog:supabasetoken"],
  f"SUPABASE_SERVICE_ROLE={v}", [v])
v = "sb_secret_" + r(B64URL, 31)
P("supabase-secret", "Supabase secret key (sb_secret_)", ["github"], f"createClient(url, '{v}')", [v])
v = "AAAA" + r(B64URL, 7) + ":APA91b" + r(B64URL, 134)
P("fcm-server-key", "Firebase Cloud Messaging server key (AAAA…:APA91b…)", ["gitleaks:gcp (fcm)", "github"],
  f"Authorization: key={v}", [v])
v = "AAAA" + r(B64URL, 7) + ":APA91b" + r(B64URL, 134)
P("fcm-server-key-bare", "FCM server key, unlabelled", ["github"], f"fcm {v}", [v])
v = "pscale_pw_" + r(B64URL, 43)
P("planetscale-pw", "PlanetScale password (pscale_pw_)", ["gitleaks:planetscale-password", "github"],
  f"mysql -h aws.connect.psdb.cloud -u x {v}", [v])
v = "pscale_tkn_" + r(B64URL, 43)
P("planetscale-token", "PlanetScale token (pscale_tkn_)", ["gitleaks:planetscale-api-token", "github"],
  f"planetscale {v}", [v])
v = r(HEX, 32)
P("datadog-api-labelled", "Datadog API key, labelled", ["gitleaks:datadog-access-token", "trufflehog:datadogtoken"],
  f"DD_API_KEY={v}", [v])
v = r(HEX, 40)
P("datadog-app-labelled", "Datadog application key, labelled", ["trufflehog:datadogtoken"], f"DD_APP_KEY={v}", [v])
v = "NRAK-" + r(UP + DG, 27)
P("newrelic-user", "New Relic user key (NRAK-)", ["gitleaks:new-relic-user-api-key", "github"], f"newrelic {v}", [v])
v = "NRII-" + r(B64URL, 32)
P("newrelic-insert", "New Relic insert key (NRII-)", ["gitleaks:new-relic-insert-key", "github"],
  f"newrelic {v}", [v])
k = r(HEX, 32)
P("sentry-dsn", "Sentry DSN (key@…ingest.sentry.io)", ["github", "trufflehog:sentrytoken"],
  f"SENTRY_DSN=https://{k}@o{r(DG, 6)}.ingest.sentry.io/{r(DG, 7)}", [k])
v = "sntrys_eyJ" + r(B64URL, 120)
P("sentry-org-token", "Sentry org auth token (sntrys_)", ["gitleaks:sentry-org-token", "github"],
  f"sentry {v}", [v])
v = "sntryu_" + r(HEX, 64)
P("sentry-user-token", "Sentry user token (sntryu_)", ["gitleaks:sentry-user-token", "github"], f"sentry {v}", [v])
v = r(HEX, 32)
P("algolia-admin-labelled", "Algolia admin key, labelled", ["gitleaks:algolia-api-key", "trufflehog:algoliaadminkey"],
  f"ALGOLIA_ADMIN_API_KEY={v}", [v])
v = "sk.eyJ1IjoiZGVtbyIsImEiOiJ" + r(B64URL, 40) + "." + r(B64URL, 22)
P("mapbox-secret", "Mapbox secret token (sk.eyJ…)", ["gitleaks:mapbox-api-token", "github", "trufflehog:mapboxsecret"],
  f"mapbox {v}", [v])
v = "lin_api_" + r(AN, 40)
P("linear", "Linear key (lin_api_)", ["gitleaks:linear-api-key", "github"], f"linear {v}", [v])
v = "secret_" + r(AN, 43)
P("notion-legacy", "Notion integration secret (secret_)", ["gitleaks:notion-api-token", "github", "trufflehog:notion"],
  f"notion = Client(auth=\"{v}\")", [v])
v = "ntn_" + r(AN, 46)
P("notion-ntn", "Notion integration token (ntn_)", ["gitleaks:notion-api-token", "github"], f"notion {v}", [v])
v = "pat" + r(AN, 14) + "." + r(HEX, 64)
P("airtable-pat", "Airtable personal access token (pat….…)", ["gitleaks:airtable-personnal-access-token", "github"],
  f"airtable {v}", [v])
v = "figd_" + r(B64URL, 40)
P("figma", "Figma token (figd_)", ["gitleaks:figma-pat", "github"], f"X-Figma-Token: {v}", [v])
v = "figd_" + r(B64URL, 40)
P("figma-bare", "Figma token, unlabelled", ["github"], f"figma {v}", [v])
v = r(AN, 24)
P("vercel-labelled", "Vercel token, labelled", ["trufflehog:vercel", "github"], f"VERCEL_TOKEN={v}", [v])
v = "nfp_" + r(AN, 36)
P("netlify-nfp", "Netlify token (nfp_)", ["github", "trufflehog:netlify"], f"netlify {v}", [v])
v = "dp.pt." + r(AN, 43)
P("doppler", "Doppler token (dp.pt.)", ["gitleaks:doppler-api-token", "github"], f"doppler {v}", [v])
v = "ops_eyJ" + r(B64URL, 250)
P("onepassword-sa", "1Password service-account token (ops_eyJ…)", ["gitleaks:1password-service-account-token", "github"],
  f"op {v}", [v])
v = "A3-" + r(UP + DG, 6) + "-" + r(UP + DG, 11) + "-" + "-".join(r(UP + DG, 5) for _ in range(3))
P("onepassword-secret-key", "1Password Secret Key (A3-…)", ["gitleaks:1password-secret-key"], f"1password {v}", [v])
v = "hvs." + r(B64URL, 90)
P("vault-hvs", "HashiCorp Vault service token (hvs.)", ["gitleaks:vault-service-token", "github"], f"vault {v}", [v])
v = "hvb." + r(B64URL, 120)
P("vault-hvb", "HashiCorp Vault batch token (hvb.)", ["gitleaks:vault-batch-token", "github"], f"vault {v}", [v])
v = "s." + r(AN, 24)
P("vault-legacy-labelled", "Vault legacy token (s.), labelled", ["gitleaks:vault-service-token"], f"VAULT_TOKEN={v}", [v])
v = r(AN, 14) + ".atlasv1." + r(B64URL, 67)
P("terraform-cloud", "Terraform Cloud token (….atlasv1.…)", ["gitleaks:hashicorp-tf-api-token", "github"],
  f'credentials "app.terraform.io" {{ token = "{v}" }}', [v])
v = "AGE-SECRET-KEY-1" + r("QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7L", 58)
P("age-secret-key", "age secret key (AGE-SECRET-KEY-1)", ["gitleaks:age-secret-key", "github"],
  f"# created: 2026-11-15\n# public key: age1{r('qpzry9x8gf2tvdw0s3jn54khce6mua7l', 58)}\n{v}", [v])
pw_b64, tok_b64, dbp, url_b64, cfg = r(B64, 16) + "==", r(B64, 40), r(B64, 20) + "=", r(B64, 48), r(B64, 32)
P("k8s-secret", "Kubernetes Secret data: values (base64)", ["github (kubernetes)", "trufflehog:kubeconfig"],
  "apiVersion: v1\nkind: Secret\nmetadata:\n  name: app\ntype: Opaque\ndata:\n"
  f"  password: {pw_b64}\n  api-token: {tok_b64}\n  db-pass: {dbp}\n  DATABASE_URL: {url_b64}\n  config: {cfg}\n",
  [pw_b64, tok_b64, dbp, url_b64, cfg])

# ---------------------------------------------------------------- keys and certificates

for kind in ("RSA PRIVATE KEY", "EC PRIVATE KEY", "DSA PRIVATE KEY", "OPENSSH PRIVATE KEY", "ENCRYPTED PRIVATE KEY",
             "PRIVATE KEY", "PGP PRIVATE KEY BLOCK"):
    body = pem_body(5)
    P("pem-" + kind.lower().replace(" ", "-"), f"PEM block: {kind}", ["gitleaks:private-key", "github",
      "trufflehog:privatekey", "detect-secrets:PrivateKeyDetector"],
      f"-----BEGIN {kind}-----\n{body}\n-----END {kind}-----", [body.split("\n")[0], body.split("\n")[-1]])
body = pem_body(3)
P("pem-truncated", "PEM private key with no END line (pasted in part)", ["gitleaks:private-key"],
  f"-----BEGIN RSA PRIVATE KEY-----\n{body}", [body.split("\n")[0], body.split("\n")[-1]])
priv, mac = r(B64, 64), r(HEX, 64)
P("putty-ppk", "PuTTY private key file (.ppk)", ["gitleaks:private-key (putty)", "detect-secrets:PrivateKeyDetector"],
  "PuTTY-User-Key-File-3: ssh-ed25519\nEncryption: none\nComment: eddsa-key-20261115\nPublic-Lines: 2\n"
  f"{r(B64, 64)}\n{r(B64, 4)}\nPrivate-Lines: 1\n{priv}\nPrivate-MAC: {mac}", [priv, mac])
d, p_, q_ = r(B64URL, 86), r(B64URL, 43), r(B64URL, 43)
P("jwk-rsa-private", "JWK private RSA members (d, p, q)", ["trufflehog:jwt (jwk)"],
  '{"kty":"RSA","kid":"k1","n":"' + r(B64URL, 86) + '","e":"AQAB","d":"' + d + '","p":"' + p_ + '","q":"' + q_ + '"}',
  [d, p_, q_])
k = r(B64URL, 43)
P("jwk-oct", "JWK symmetric key (k)", ["trufflehog:jwt (jwk)"], '{"kty": "oct", "alg": "HS256", "k": "' + k + '"}', [k])
pw = r(AN, 10) + "!"
P("sshpass-p", "sshpass -p password", ["github (generic)"], f"sshpass -p '{pw}' ssh deploy@10.0.0.5", [pw])
pw = "hunterpass"
P("sshpass-env", "SSHPASS= environment value", ["github (generic)"], f"SSHPASS={pw} sshpass -e ssh deploy@host", [pw])
pw = "correcthorse"
P("pgpassword-env", "PGPASSWORD= environment value", ["github (generic)"], f"PGPASSWORD={pw} psql -h db -U app", [pw])
pw = "hunterpass"
P("netrc-line", ".netrc one line (password with no digit)", ["trufflehog:netrc"],
  f"machine api.example.com login deploy password {pw}", [pw])
pw = "bluewhale"
P("netrc-multiline", ".netrc stanza over lines", ["trufflehog:netrc"],
  f"machine git.example.com\n  login deploy\n  password {pw}\n", [pw])
pw = "Tr0ub4dor&3"
P("pgpass", ".pgpass line host:port:db:user:password", ["github (generic)"],
  f"db.example.com:5432:appdb:appuser:{pw}", [pw])
h = "$apr1$" + r(AN, 8) + "$" + r(AN + "./", 22)
P("htpasswd-apr1", "htpasswd line (Apache MD5 hash)", ["github (generic)"], f"admin:{h}", [h])
h = "$2y$10$" + r(AN + "./", 53)
P("bcrypt-hash", "bcrypt password hash", ["github (generic)"], f"users.password_hash = '{h}'", [h])
h = "$6$" + r(AN, 16) + "$" + r(AN + "./", 86)
P("shadow-sha512", "/etc/shadow sha512-crypt hash", ["github (generic)"], f"root:{h}:19700:0:99999:7:::", [h])
v = "npm_" + r(AN, 36)
P("npmrc-authtoken", ".npmrc _authToken", ["github"], f"//npm.pkg.github.com/:_authToken={v}", [v])
v = r(B64, 32)
P("docker-config-auth", "Docker config.json auth", ["github", "trufflehog:dockerhub"],
  '{"auths": {"https://index.docker.io/v1/": {"auth": "' + v + '"}}}', [v])
v = r(B64URL, 60)
P("docker-identitytoken", "Docker config.json identitytoken", ["github"],
  '{"auths": {"registry.example.com": {"identitytoken": "' + v + '"}}}', [v])
v = r(AN, 20)
P("git-credential-url", "git credential URL https://user:token@host", ["gitleaks:generic (url)", "github"],
  f"https://deploy:{v}@git.example.com/acme/app.git", [v])
v = "ghs_" + r(AN, 36)
P("git-x-access-token", "git URL x-access-token:ghs_…", ["github"], f"https://x-access-token:{v}@github.com/acme/app", [v])
v = r(B64, 28)
P("basic-auth-header", "Authorization: Basic …", ["github (generic)", "detect-secrets:BasicAuthDetector"],
  f"Authorization: Basic {v}", [v])
v = r(B64URL, 40)
P("bearer-header", "Authorization: Bearer …", ["github (generic)"], f"curl -H 'Authorization: Bearer {v}' https://api.example.com", [v])
v = r(AN, 32)
P("x-api-key-header", "X-API-Key header", ["github (generic)"], f"X-API-Key: {v}", [v])
v = r(AN, 32)
P("api-key-header", "Api-Key header", ["github (generic)"], f"api-key: {v}", [v])

# ---------------------------------------------------------------- crypto wallets

words12 = "remind cushion zebra ankle invite crane mimic oyster lunar fabric whisper drift"
P("seed-phrase-labelled", "BIP-39 seed phrase after a label", ["trufflehog:mnemonic? (no; generic)"],
  f"seed phrase: {words12}", words12.split())
words24 = ("guitar velvet absorb picnic stable ozone tribe cradle hollow marine spider wagon "
           "anchor ribbon frozen debate kiwi lemon cotton grief harsh basket elbow quiz")
P("mnemonic-labelled", "BIP-39 mnemonic (24 words), quoted after mnemonic =", ["generic"],
  f'MNEMONIC = "{words24}"', words24.split())
P("recovery-phrase-labelled", "Secret Recovery Phrase (MetaMask wording)", ["generic"],
  f"Secret Recovery Phrase\n{words12}", words12.split())
P("seed-phrase-bare", "BIP-39 seed phrase with no label", ["generic"], f"{words12}", words12.split(),
  limit="no shape: twelve ordinary English words; catching them needs the BIP-39 word list (not in the crate)")
v = "0x" + r(HEX, 64)
P("eth-private-key-labelled", "hex private key after a label", ["gitleaks:generic", "trufflehog:ethereum"],
  f"PRIVATE_KEY={v}", [v[2:]])
v = r(HEX, 64)
P("eth-private-key-prose", "hex private key in prose after 'private key:'", ["gitleaks:generic"],
  f"wallet private key: {v}", [v])
v = "5" + RNG.choice("HJK") + r(B58, 49)
P("wif-uncompressed", "Bitcoin WIF private key (5H/5J/5K, 51 chars)", ["trufflehog:bitcoin? (generic)"],
  f"importprivkey {v}", [v])
v = RNG.choice("KL") + r(B58, 51)
P("wif-compressed", "Bitcoin WIF private key (K/L, 52 chars)", ["generic"], f"wif: {v}", [v])
v = r(HEX, 64)
P("eth-private-key-bare", "64-hex private key with no label", ["generic"], f"{v}", [v],
  limit="no shape: 64 hex characters are exactly a SHA-256 digest; a label is needed")

# ---------------------------------------------------------------- cards and codes (FERB's own rules)

P("card-expiry-cvv", "payment card, expiry and CVV", ["ferb", "github (generic)"],
  "card 4111 1111 1111 1111 exp 12/29 cvv 123", ["4111 1111 1111 1111", "12/29", "123"])
P("otp-sentence", "one-time code in a sentence", ["ferb"], "Your verification code is 482913.", ["482913"])
P("upi-pin", "UPI PIN after its label", ["ferb"], "UPI PIN: 4821", ["4821"])

# ---------------------------------------------------------------- generic labels, forms and syntaxes

def G(id_: str, text: str, value: str, shape: str = "", limit: str = "") -> None:
    P("label-" + id_, shape or f"labelled value: {id_}", ["gitleaks:generic-api-key", "detect-secrets:KeywordDetector"],
      text, [value], limit)


G("password-eq", "password=hunter2", "hunter2")
G("passwd-colon", "passwd: hunter2", "hunter2")
G("pwd-eq", "pwd=hunter2", "hunter2")
G("db-pass", "DB_PASS=hunterpass", "hunterpass")
G("smtp-pass-yaml", "smtp_pass: bluewhale", "bluewhale")
G("pw-suffix", "ADMIN_PW=bluewhale", "bluewhale")
G("secret-eq", "secret=bluewhale", "bluewhale")
G("token-eq", "token=bluewhale", "bluewhale")
G("api-key-prose", "the api key: Zx8Kq2Lm9Vb4", "Zx8Kq2Lm9Vb4")
G("apikey-eq", "apikey=Zx8Kq2Lm9Vb4", "Zx8Kq2Lm9Vb4")
G("access-key-prose", "access key: Zx8Kq2Lm9Vb4Nc7R", "Zx8Kq2Lm9Vb4Nc7R")
G("auth-eq", "auth=Zx8Kq2Lm9Vb4", "Zx8Kq2Lm9Vb4")
G("credential-eq", "credential=Zx8Kq2Lm9Vb4", "Zx8Kq2Lm9Vb4")
G("private-key-prose", "private key: Zx8Kq2Lm9Vb4Nc7R", "Zx8Kq2Lm9Vb4Nc7R")
G("client-secret-prose", "client secret: bluewhale", "bluewhale")
G("client-secret-snake", "client_secret=bluewhale", "bluewhale")
G("signing-key-eq", "signing_key = bluewhale", "bluewhale")
G("encryption-key-colon", "encryptionKey: bluewhale", "bluewhale")
G("passphrase-colon", "passphrase: correct horse", "correct")
G("pin-prose", "my PIN is 4821", "4821")
G("camel", "dbPassword = 'bluewhale'", "bluewhale")
G("kebab", "--db-password=bluewhale", "bluewhale")
G("screaming", "DB_PASSWORD=bluewhale", "bluewhale")
G("spaced", "db password: bluewhale", "bluewhale")
G("prefix-suffix-env", "MY_APP_DB_PASSWORD_PROD=bluewhale", "bluewhale", "label with an environment suffix (_PROD)")
G("numbered", "DB_PASSWORD2=bluewhale", "bluewhale", "label with a number after the word")
G("suffix-old", "API_TOKEN_OLD=bluewhale", "bluewhale", "label with a qualifier suffix (_OLD)")
G("glued-pgpassword", "PGPASSWORD=bluewhale", "bluewhale", "glued label word (PGPASSWORD)")
G("glued-authtoken", "AUTHTOKEN=bluewhale", "bluewhale", "glued label word (AUTHTOKEN)")
G("json", '{"user": "ana", "password": "bluewhale"}', "bluewhale", "JSON")
G("yaml", "database:\n  user: ana\n  password: bluewhale\n", "bluewhale", "YAML")
G("toml", 'password = "bluewhale"', "bluewhale", "TOML")
G("ini", "[db]\npassword = bluewhale", "bluewhale", "INI")
G("export", "export DB_PASSWORD='bluewhale'", "bluewhale", "shell export")
G("dotenv", "DB_PASSWORD=bluewhale\nDB_HOST=db", "bluewhale", "dotenv")
G("xml-element", "<password>bluewhale</password>", "bluewhale", "XML element")
G("xml-attr", '<user name="ana" password="bluewhale"/>', "bluewhale", "XML attribute")
G("dotnet-appsetting", '<add key="DbPassword" value="bluewhale" />', "bluewhale", ".NET appSettings key/value")
G("php-arrow", "'password' => 'bluewhale',", "bluewhale", "PHP =>")
G("arrow", "password -> bluewhale", "bluewhale", "->",
  limit="-> is member access in C and PHP (token->type): it names no value")
G("go-walrus", 'password := "bluewhale"', "bluewhale", "Go :=")
G("python-kwarg", 'connect(user="ana", password="bluewhale")', "bluewhale", "keyword argument")
G("call-positional", 'login("ana", "bluewhale")', "bluewhale", "positional arguments of login()",
  limit="no label: the argument's position names it only to someone who knows the function")
G("setter-call", 'setPassword("bluewhale")', "bluewhale", "setter call setPassword(...)")
G("http-form", "user=ana&password=p%40ssword&remember=1", "p%40ssword", "form body, percent-encoded value")
G("german", "Passwort: geheim", "geheim", "German Passwort")
G("german-kennwort", "Kennwort: geheim", "geheim", "German Kennwort")
G("spanish", "contraseña: secreta", "secreta", "Spanish contraseña")
G("portuguese", "senha: segredo", "segredo", "Portuguese senha")
G("french", "mot de passe : secret1", "secret1", "French mot de passe")
G("hindi-devanagari", "पासवर्ड: bluewhale", "bluewhale", "Hindi पासवर्ड")
G("misspelled", "pasword: bluewhale", "bluewhale", "misspelling pasword")
v = r(AN, 24)
P("unlabelled-high-entropy", "high-entropy string with no label", ["detect-secrets:Base64HighEntropyString"],
  f"here it is: {v}", [v],
  limit="no label and no vendor prefix: a random-looking run is also an id, a hash, a nonce or a file name")
P("unlabelled-short", "short password with no label", ["generic"], "use bluewhale to sign in", ["bluewhale"],
  limit="no label word next to it: 'use X to sign in' is prose")
v = r(B64, 40)
P("base64-wrapped", "a secret wrapped in base64 with no label", ["generic"], f"blob {v}", [v],
  limit="base64 of anything looks like base64 of a secret")


# ---------------------------------------------------------------- more syntaxes the added rules read

v = "EAA" + r(AN, 180)
P("facebook-token", "Facebook access token (EAA…)", ["gitleaks:facebook-access-token", "github",
  "trufflehog:facebookoauth"], f"graph call with {v}", [v])
pw = r(AN, 12)
P("curl-user", "curl -u user:password", ["github (generic)"], f"curl -u admin:{pw} https://api.example.com/v1", [pw])
pw = r(AN, 12)
P("cli-password-space", "--password VALUE (space-separated)", ["github (generic)"],
  f"heroku login --email a@example.com --password {pw}", [pw])
pw = r(AN, 12)
P("docker-login-p", "docker login -p password", ["github (generic)"],
  f"docker login registry.example.com -u ci -p {pw}", [pw])
pw = r(AN, 12)
P("html-input-value", "HTML input name=password value=…", ["github (generic)"],
  f'<input type="password" name="password" value="{pw}">', [pw])
P("ruby-symbol-arrow", "Ruby :password => …", ["gitleaks:generic-api-key"], '{ :password => "bluewhale" }', ["bluewhale"])
v = r(AN, 32)
P("toml-api-key-prod", "a key label with an environment suffix (api_key_prod)", ["gitleaks:generic-api-key"],
  f'api_key_prod = "{v}"', [v])


# ---------------------------------------------------------------- GATE-598's opposite risk: values that look like an
# exemption but are not, kept masked

P("pin-repeated-digits", "a PIN of one repeated digit", ["ferb"], "PIN: 1111", ["1111"])
P("password-capitalised-in-parens", "a capitalised password in parentheses (no type)", ["ferb"],
  "(password: Hunter2) for the router", ["Hunter2"])
P("password-type-prefix", "a password that starts like a type name", ["ferb"], "password: str-ong9", ["str-ong9"])
P("password-dotted", "a dotted password with no lookup root", ["ferb"], "password: my.pass.word", ["my.pass.word"])
P("password-repeated-letter", "a run of one letter that is not x", ["ferb"], "password: zzzz9", ["zzzz9"])
P("password-mixed-caps", "a password in caps and lower case with underscores", ["ferb"],
  "DB_PASSWORD=SuperSecret_2024", ["SuperSecret_2024"])
P("password-compared-literal", "a literal compared with a password in code", ["gitleaks:generic-api-key"],
  'if password == "hunter2!x": grant()', ["hunter2!x"])

# ---------------------------------------------------------------- personal identifiers, category (i) (GATE-599)
# Sources: presidio = Microsoft Presidio's recognisers; dlp = Google Cloud Sensitive Data Protection infoTypes;
# macie = AWS Macie managed data identifiers (each list read 2026-11-15); purview = Microsoft Purview sensitive
# information types (from memory, not re-read).

sys.dont_write_bytecode = True  # no __pycache__ beside the fixture
import pii_make  # noqa: E402

G_ = pii_make.Gen(RNG)


def ID(id_: str, kind: str, sources: list[str], text: str, value: str) -> None:
    P("pii-" + id_, kind, sources, text, [value])


v = G_.aadhaar()
ID("aadhaar-grouped", "Aadhaar number, 4-4-4 (Verhoeff)", ["presidio:IN_AADHAAR", "dlp:INDIA_AADHAAR_INDIVIDUAL", "macie:INDIA_AADHAAR_NUMBER"], f"Aadhaar card {v} issued", v)
v = G_.aadhaar_plain()
ID("aadhaar-labelled", "Aadhaar number, plain after its label", ["presidio:IN_AADHAAR"], f"my aadhaar no is {v}", v)
v = G_.aadhaar_plain()
ID("aadhaar-hindi", "Aadhaar number after आधार", ["presidio:IN_AADHAAR"], f"आधार संख्या: {v}", v)
v = G_.vid()
ID("aadhaar-vid", "Aadhaar VID (16, Verhoeff), labelled", ["dlp:INDIA_AADHAAR_INDIVIDUAL (VID: memory)"], f"VID: {v}", v)
v = G_.pan()
ID("pan", "PAN (AAAAA9999A)", ["presidio:IN_PAN", "dlp:INDIA_PAN_INDIVIDUAL", "macie:INDIA_PERMANENT_ACCOUNT_NUMBER"], f"my PAN is {v}, file it", v)
v = G_.epic()
ID("epic", "voter ID (EPIC), labelled", ["presidio:IN_VOTER"], f"Voter ID: {v}", v)
v = G_.ssn()
ID("ssn", "US SSN (area/group/serial rules)", ["presidio:US_SSN", "dlp:US_SOCIAL_SECURITY_NUMBER", "macie:USA_SOCIAL_SECURITY_NUMBER"], f"SSN {v} on file", v)
v = G_.ssn().replace("-", "")
ID("ssn-plain", "US SSN plain after its label", ["presidio:US_SSN"], f"Social Security Number: {v}", v)
v = G_.itin()
ID("itin", "US ITIN", ["presidio:US_ITIN", "dlp:US_INDIVIDUAL_TAXPAYER_IDENTIFICATION_NUMBER"], f"ITIN {v}", v)
v = G_.mbi()
ID("mbi", "US Medicare Beneficiary Identifier", ["presidio:US_MBI", "dlp:US_MEDICARE_BENEFICIARY_ID_NUMBER", "macie:USA_MEDICARE_BENEFICIARY_IDENTIFIER"], f"Medicare card {v}", v)
v = G_.routing()
ID("routing", "US ABA routing number (3-7-1), labelled", ["dlp:US_BANK_ROUTING_MICR", "macie:BANK_ACCOUNT_NUMBER"], f"Routing number: {v}", v)
v = G_.d(10)
ID("us-account", "US bank account, labelled", ["presidio:US_BANK_NUMBER", "macie:BANK_ACCOUNT_NUMBER"], f"Account number: {v}", v)
v = G_.nino()
ID("nino", "UK National Insurance number", ["presidio:UK_NINO", "dlp:UK_NATIONAL_INSURANCE_NUMBER", "macie:UK_NATIONAL_INSURANCE_NUMBER"], f"NI number {v}", v)
v = G_.nhs()
ID("nhs", "UK NHS number (mod 11), labelled", ["presidio:UK_NHS", "dlp:UK_NATIONAL_HEALTH_SERVICE_NUMBER", "macie:UK_NHS_NUMBER"], f"NHS No: {v}", v)
v = G_.uk_driving()
ID("uk-driving", "UK driving licence (16, DVLA layout)", ["presidio:UK_DRIVING_LICENCE", "dlp:UK_DRIVERS_LICENSE_NUMBER"], f"licence {v} valid", v)
v = "12-34-56"
ID("uk-sort-code", "UK sort code, labelled", ["macie:UK_BANK_ACCOUNT_NUMBER"], f"Sort code: {v}", v)
v = G_.sin()
ID("ca-sin", "Canada SIN (Luhn), 3-3-3", ["presidio:CA_SIN", "dlp:CANADA_SOCIAL_INSURANCE_NUMBER", "macie:CANADA_SOCIAL_INSURANCE_NUMBER"], f"SIN {v}", v)
v = G_.dni()
ID("es-dni", "Spain DNI (mod 23)", ["presidio:ES_NIF", "dlp:SPAIN_DNI_NUMBER", "macie:SPAIN_DNI_NUMBER"], f"con DNI {v} firmó", v)
v = G_.nie()
ID("es-nie", "Spain NIE", ["presidio:ES_NIE", "dlp:SPAIN_NIE_NUMBER"], f"NIE {v}", v)
v = G_.codice_fiscale()
ID("it-cf", "Italy codice fiscale (check char)", ["presidio:IT_FISCAL_CODE", "dlp:ITALY_FISCAL_CODE"], f"codice {v}", v)
v = G_.nir()
ID("fr-nir", "France NIR (key mod 97)", ["dlp:FRANCE_NIR", "macie:FRANCE_NATIONAL_IDENTIFICATION_NUMBER"], f"n° {v}", v)
v = G_.personnummer()
ID("se-pnr", "Sweden personnummer (Luhn)", ["presidio:SE_PERSONNUMMER", "dlp:SWEDEN_NATIONAL_ID_NUMBER"], f"pnr {v}", v)
v = G_.hetu()
ID("fi-hetu", "Finland henkilötunnus (mod 31)", ["presidio:FI_PERSONAL_IDENTITY_CODE", "dlp:FINLAND_NATIONAL_ID_NUMBER"], f"hetu {v}", v)
v = G_.be_nrn()
ID("be-nrn", "Belgium national register number (mod 97)", ["dlp:BELGIUM_NATIONAL_ID_CARD_NUMBER"], f"NN {v}", v)
v = G_.steuer_id()
ID("de-steuer", "Germany Steuer-ID (ISO 7064), labelled", ["presidio:DE_TAX_ID", "dlp:GERMANY_TAXPAYER_IDENTIFICATION_NUMBER"], f"Steuer-ID: {v}", v)
v = G_.ausweis()
ID("de-ausweis", "Germany Personalausweis (7-3-1), labelled", ["presidio:DE_ID_CARD", "dlp:GERMANY_IDENTITY_CARD_NUMBER"], f"Ausweisnummer {v}", v)
v = G_.bsn()
ID("nl-bsn", "Netherlands BSN (11-proef), labelled", ["dlp:NETHERLANDS_BSN_NUMBER"], f"BSN: {v}", v)
v = G_.pesel()
ID("pl-pesel", "Poland PESEL, labelled", ["presidio:PL_PESEL", "dlp:POLAND_PESEL_NUMBER"], f"PESEL {v}", v)
v = G_.cpr()
ID("dk-cpr", "Denmark CPR, labelled", ["dlp:DENMARK_CPR_NUMBER"], f"CPR-nr.: {v}", v)
v = G_.pt_nif()
ID("pt-nif", "Portugal NIF (mod 11), labelled", ["dlp:PORTUGAL_CDC_NUMBER (NIF: memory)"], f"NIF {v}", v)
v = G_.ppsn()
ID("ie-ppsn", "Ireland PPSN (mod 23), labelled", ["dlp:IRELAND_PPSN"], f"PPS number: {v}", v)
v = G_.cpf()
ID("br-cpf", "Brazil CPF (two mod-11 digits)", ["dlp:BRAZIL_CPF_NUMBER", "macie:BRAZIL_CPF_NUMBER"], f"CPF {v}", v)
v = G_.curp()
ID("mx-curp", "Mexico CURP (check digit)", ["dlp:MEXICO_CURP_NUMBER", "macie:MEXICO_CURP_NUMBER"], f"CURP {v}", v)
v = G_.rfc()
ID("mx-rfc", "Mexico RFC, labelled", ["macie:MEXICO_INDIVIDUAL_RFC_NUMBER"], f"RFC: {v}", v)
v = G_.cuil()
ID("ar-cuil", "Argentina CUIL (mod 11)", ["macie:ARGENTINA_INDIVIDUAL_TAX_IDENTIFICATION_NUMBER"], f"CUIL {v}", v)
v = f"{G_.rng.randint(20, 45)}.{G_.d(3)}.{G_.d(3)}"
ID("ar-dni", "Argentina DNI, labelled", ["dlp:ARGENTINA_DNI_NUMBER", "macie:ARGENTINA_DNI_NUMBER"], f"DNI {v}", v)
v = G_.rut()
ID("cl-rut", "Chile RUT (mod 11)", ["dlp:CHILE_CDI_NUMBER", "macie:CHILE_RUT_NUMBER"], f"RUT {v}", v)
v = G_.china_id()
ID("cn-rid", "China resident identity number (ISO 7064 MOD 11-2)", ["dlp:CHINA_RESIDENT_ID_NUMBER"], f"身份证号码 {v}", v)
v = G_.my_number()
ID("jp-mynumber", "Japan My Number, labelled", ["dlp:JAPAN_INDIVIDUAL_NUMBER"], f"マイナンバー: {v}", v)
v = G_.kr_rrn()
ID("kr-rrn", "South Korea resident registration number", ["presidio:KR_RRN", "dlp:KOREA_RRN"], f"주민등록번호 {v}", v)
v = G_.nric()
ID("sg-nric", "Singapore NRIC/FIN (check letter)", ["presidio:SG_NRIC_FIN", "dlp:SINGAPORE_NATIONAL_REGISTRATION_ID_NUMBER"], f"NRIC {v}", v)
v = G_.hkid()
ID("hk-hkid", "Hong Kong HKID (mod 11), labelled", ["dlp:HONG_KONG_ID_NUMBER"], f"HKID: {v}", v)
v = G_.taiwan()
ID("tw-id", "Taiwan national ID (check digit)", ["dlp:TAIWAN_ID_NUMBER"], f"身分證字號 {v}", v)
v = G_.mykad()
ID("my-mykad", "Malaysia MyKad", ["purview:Malaysia identity card number (memory)"], f"MyKad {v}", v)
v = G_.nik()
ID("id-nik", "Indonesia NIK, labelled", ["dlp:INDONESIA_NIK_NUMBER"], f"NIK: {v}", v)
v = G_.cnic()
ID("pk-cnic", "Pakistan CNIC (5-7-1)", ["purview:Pakistan CNIC (memory)"], f"CNIC {v}", v)
v = G_.lk_nic()
ID("lk-nic", "Sri Lanka NIC (old, day of year + V)", ["purview (memory)"], f"NIC {v}", v)
v = G_.emirates()
ID("ae-eid", "UAE Emirates ID (784, Luhn)", ["purview:U.A.E. identity card number (memory)"], f"Emirates ID {v}", v)
v = G_.saudi()
ID("sa-id", "Saudi national ID / Iqama (Luhn), labelled", ["purview:Saudi Arabia National ID (memory)"], f"Iqama: {v}", v)
v = G_.il_id()
ID("il-id", "Israel ID (check digit), labelled", ["dlp:ISRAEL_IDENTITY_CARD_NUMBER"], f"ת.ז. {v}", v)
v = G_.tckn()
ID("tr-tckn", "Turkey TC Kimlik (two checks), labelled", ["presidio:TR_NATIONAL_ID", "dlp:TURKEY_ID_NUMBER"], f"TC Kimlik No: {v}", v)
v = G_.za_id()
ID("za-id", "South Africa ID (date + Luhn)", ["presidio:ZA_ID_NUMBER", "dlp:SOUTH_AFRICA_ID_NUMBER"], f"ID number: {v}", v)
v = G_.d(11)
ID("ng-nin", "Nigeria NIN, labelled", ["presidio:NG_NIN"], f"NIN: {v}", v)
v = G_.kra()
ID("ke-kra", "Kenya KRA PIN, labelled", ["purview (memory)"], f"KRA PIN {v}", v)
v = G_.snils()
ID("ru-snils", "Russia SNILS (check)", ["purview (memory)"], f"СНИЛС {v}", v)
v = G_.inn()
ID("ru-inn", "Russia personal INN (two checks), labelled", ["purview (memory)"], f"ИНН {v}", v)
v = G_.tfn()
ID("au-tfn", "Australia TFN (mod 11), labelled", ["presidio:AU_TFN", "dlp:AUSTRALIA_TAX_FILE_NUMBER"], f"Tax File Number: {v}", v)
v = G_.au_medicare()
ID("au-medicare", "Australia Medicare (check), labelled", ["presidio:AU_MEDICARE", "dlp:AUSTRALIA_MEDICARE_NUMBER"], f"Medicare card: {v}", v)
v = G_.iban("DE", 18)
ID("iban-de", "IBAN (mod 97), paper form", ["presidio:IBAN_CODE", "dlp:IBAN_CODE", "macie:GERMANY_BANK_ACCOUNT_NUMBER"], f"IBAN {v} BIC DEUTDEFF", v)
v = G_.iban("GB", 18).replace(" ", "")
ID("iban-gb", "IBAN (mod 97), compact", ["presidio:IBAN_CODE", "dlp:IBAN_CODE"], f"pay to {v} today", v)
v = G_.d(12)
ID("in-account", "India bank account after खाता संख्या", ["dlp:FINANCIAL_ACCOUNT_NUMBER"], f"खाता संख्या {v}, IFSC SBIN0001234", v)
v = G_.d(11)
ID("in-account-en", "India bank account after A/c No.", ["dlp:FINANCIAL_ACCOUNT_NUMBER"], f"A/c No. {v} IFSC HDFC0000123", v)
v = G_.u(1) + G_.d(7)
ID("passport-en", "passport number, labelled", ["presidio:US_PASSPORT/IN_PASSPORT", "dlp:*_PASSPORT", "macie:*_PASSPORT_NUMBER"], f"Passport No: {v}", v)
v = G_.u(2) + G_.d(7)
ID("passport-es", "passport number after pasaporte", ["dlp:SPAIN_PASSPORT"], f"pasaporte {v}", v)
v = G_.d(2) + " " + G_.d(2) + " " + G_.d(6)
ID("passport-ru", "Russian passport after паспорт (spaced)", ["dlp:RUSSIA_PASSPORT"], f"паспорт {v}", v)
v = G_.u(1) + G_.d(7)
ID("passport-hi", "passport number after पासपोर्ट", ["dlp:INDIA_PASSPORT"], f"पासपोर्ट नंबर {v}", v)
v = "MH12 " + G_.d(4) + G_.d(7)
ID("in-dl", "India driving licence, labelled", ["macie:INDIA_DRIVERS_LICENSE"], f"Driving Licence No: {v}", v)
v = G_.u(1) + G_.d(7)
ID("us-dl", "US driver's license, labelled", ["presidio:US_DRIVER_LICENSE", "macie:DRIVERS_LICENSE"], f"Driver's License: {v}", v)
v = G_.d(10)
ID("cedula", "national ID after cédula", ["dlp:COLOMBIA_CDC_NUMBER"], f"Cédula: {v}", v)
v = "W" + G_.d(9)
ID("health-member", "health insurance member ID, labelled", ["presidio:US_HEALTH_INSURANCE_MEMBER_ID", "dlp:MEDICAL_ID"], f"Member ID: {v}", v)

# ---------------------------------------------------------------- negatives: must come back unchanged

KP = "the Python label rule: a value after a password/token/key label is masked whatever it is"

N("git-sha", "40-hex git commit", "commit 3f2a9c1e4b7d8a6f0e5c2b1a9d8e7f6c5b4a3d2e\nAuthor: Dev <dev@example.com>")
N("git-short-sha", "short SHAs in a log", "63369cb3 chore(kb): regenerate rules\n8c3e7321 docs: handoff")
N("uuid", "UUIDs", "request_id=7c9e6679-7425-40de-944b-e07fc1f90ae7 trace 550e8400-e29b-41d4-a716-446655440000")
N("md5-lockfile", "MD5 and SHA-256 checksums", "d41d8cd98f00b204e9800998ecf8427e  empty.txt\n"
  "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  empty.txt")
N("cargo-lock", "Cargo.lock checksum", 'name = "serde"\nversion = "1.0.210"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n'
  'checksum = "c8e3592472072e6e22e0a54d5904d9febf8508f65fb8552499a1abc7d1078c3a"')
N("docker-digest", "sha256: image digest", "image: ghcr.io/acme/app@sha256:4d6e1f3c2b8a9e7f0d5c6b4a3e2f1d0c9b8a7f6e5d4c3b2a1f0e9d8c7b6a5f4e")
N("npm-integrity", "npm integrity sha512", '"integrity": "sha512-9NET910DNaIPngYnLLPeg+Ogzqsi9uM4mSboU5y6p8S5DzMTVEsJZrawi+BoDNUVBa2DhJqQYUFvMMMRa1Wj2Q=="')
N("uv-lock-hash", "uv.lock hash", 'hash = "sha256:9e1ffd3c2e8f0f4a7b5d6c9e8a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d"')
N("data-uri", "base64 image data URI", "<img src=\"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==\">")
N("minified-js", "minified JavaScript", "!function(e,t){\"object\"==typeof exports?module.exports=t():e.lib=t()}(this,function(){var n=0;return{id:function(){return n++}}});")
N("css-colours", "CSS colours", ".btn{color:#1a2b3c;background:#ffffff;border-color:#a1b2c3d4}")
N("versions", "version strings", "node v20.11.1, rustc 1.83.0 (90b35a623 2024-11-26), python 3.12.4")
N("timestamps", "ISO timestamps", "2026-11-15T10:30:00Z started; finished 2026-11-15T10:31:12.482913+05:30")
N("phone", "phone numbers", "Call +91 98765 43210 or (415) 555-0134")
N("order-ids", "order ids", "Order #402-8812931-7720331 shipped; invoice INV-2026-004821")
N("long-path", "long file paths", "/Users/dev/Library/Caches/com.example.app/fsCachedData/7C9E6679-7425-40DE-944B-E07FC1F90AE7")
N("url-params", "URL with ordinary query params", "https://www.example.com/search?q=rust+regex&page=2&sort=desc&utm_source=newsletter")
N("ident-tokenizer", "identifier tokenizer", "tokenizer = AutoTokenizer.from_pretrained(model_name)")
N("ident-keyboard", "identifier keyboard", "keyboard.press(Key.enter)")
N("ident-password-field", "identifier password_field", "password_field = page.locator('#password')")
N("max-tokens", "max_tokens=4096", "response = client.create(model=m, max_tokens=4096)")
N("react-key", "key={item.id}", "<Row key={item.id} value={item.value} />")
N("primary-key", '"primary_key": 12', '{"primary_key": 12, "name": "users"}')
N("token-count", "token_count: 512", "token_count: 512")
N("password-min-length", "PASSWORD_MIN_LENGTH = 8", "PASSWORD_MIN_LENGTH = 8")
N("secretary", "secretary", "The secretary filed the minutes.")
N("api-key-name", 'api_key_name = "OPENAI_API_KEY"', 'api_key_name = "OPENAI_API_KEY"')
N("empty-password-check", 'if (password === "")', 'if (password === "") { return false; }')
N("password-type-hint", "password: str", "def login(user: str, password: str) -> bool:")
N("password-input-doc", "Password: <input> in docs", "Password: <input type=\"password\">")
N("placeholder-your-key", "YOUR_API_KEY_HERE", "OPENAI_API_KEY=YOUR_API_KEY_HERE")
N("placeholder-x", "xxxxxxxx", "password: xxxxxxxx")
N("placeholder-angle", "<token>", "token=<token>")
N("placeholder-env", "${API_KEY}", "api_key=${API_KEY}")
N("process-env", "process.env.API_KEY", "const apiKey = process.env.API_KEY;")
N("os-environ", 'os.environ["API_KEY"]', 'api_key = os.environ["API_KEY"]')
N("prose-password", "prose about passwords", "Reset your password from the settings page if you forgot it.")
N("prose-token", "prose about tokens", "The model counts tokens before each call.")
N("sizes", "table of sizes", "RAM 16GB, SSD 512GB, swap 2048MB")
N("ip-mac", "IP and MAC addresses", "host 192.168.1.24 (fe80::1c2b:3d4e:5f60:7182) mac a4:83:e7:1b:2c:3d")
N("ssh-public-key", "ssh public key line", "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGq8Zx2Lm9Vb4Nc7Rt1Yw5Pj3Kq8Lm2Vb9Nc4Rt7Yw dev@laptop")
N("pem-public-key", "PEM public key block", "-----BEGIN PUBLIC KEY-----\nMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEq8Zx2Lm9Vb4Nc7Rt1Yw5Pj3Kq8Lm2Vb\n-----END PUBLIC KEY-----")
N("pem-certificate", "PEM certificate block", "-----BEGIN CERTIFICATE-----\nMIIBszCCAVmgAwIBAgIUq8Zx2Lm9Vb4Nc7Rt1Yw5Pj3Kq8Lm2Vb9Nc4\n-----END CERTIFICATE-----")
N("stripe-publishable", "Stripe publishable key (public by design)", "Stripe('pk_live_" + r(AN, 99) + "')")
N("razorpay-key-id", "Razorpay key id (public by design, shown in checkout)", "key: 'rzp_live_" + r(AN, 14) + "'")
N("mapbox-public", "Mapbox public token pk.eyJ (public by design)", "mapboxgl.accessToken = 'pk.eyJ1IjoiZGVtbyIsImEiOiJ" + r(B64URL, 40) + "." + r(B64URL, 22) + "';")
N("aws-arn", "AWS ARN and account id", "arn:aws:iam::123456789012:role/deploy")
N("rust-code", "Rust code with key/token words", "let key = map.get(&token_id).ok_or(Error::MissingKey)?;\nfor (k, v) in secrets_index.iter() {}")
N("yaml-config", "ordinary YAML config", "server:\n  port: 5432\n  user: admin\n  timeout: 30s")
N("port-user-prose", "the port and user prose", "the port is 5432 and the user is admin")
N("test-summary", "test runner summary", "test result: ok. 129 passed; 0 failed; 3 ignored; pass=129 fail=0")
N("json-pass-count", "JSON counts with pass", '{"pass": 12, "fail": 0, "total_pass": 12}')
N("bypass-word", "words containing pass", "bypass=true compass_heading=270 passport_country=IN")
N("hex-color-long", "a long hex id in a URL path", "https://github.com/acme/app/commit/3f2a9c1e4b7d8a6f0e5c2b1a9d8e7f6c5b4a3d2e")
N("mdp", "ML code with mdp", "mdp = MDP(states=s, actions=a)")
N("keyed-lookups", "dict lookups by key", "value = cache[key]; key_count = len(keys)")
N("base58-short", "a Solana address (base58, 44)", "send to 7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU")
N("ipfs-cid", "IPFS CID", "ipfs://QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG")
N("sk-word", "words starting sk-", "sk-learn and sk-image are libraries")
N("key-dash-hex", "cache-key-<md5>", "cache-key-d41d8cd98f00b204e9800998ecf8427e")
N("dollar-shell", "shell positional args", 'echo "$1$2$3" && printf "%s$1$" foo')
N("password-reset-url", "a password reset page URL", "https://accounts.example.com/password/reset?step=2")
N("machine-word", "the word machine near password in prose", "Each machine needs a password policy.")
N("colon-fields", "a colon-separated log line", "2026:11:15:10:30 job:42:done")

N("js-arrow", "JavaScript arrow function with a token parameter", "const clean = tokens.map(token => token.trim());")
N("c-member", "C member access on token", "if (token->type == TOKEN_EOF) return secret->len;")
N("go-expr", "Go := with an expression", "token := strings.TrimSpace(t)")
N("help-flags", "a CLI help text", "  --password TEXT    The password to use\n  --token string     API token\n  --token <token>    the token")
N("html-label", "an HTML label", "<label for=\"password\">Password</label>")
N("random-seed", "a random seed", "seed: 42\nnp.random.seed(1234)")
N("seed-prose", "prose with the word seed", "The seed words below were chosen for the garden: tomato basil pepper onion garlic")
N("pgpass-like", "a log with colons", "localhost:5432 accepted connection from app")
N("netrc-prose", "prose with machine and password", "Each machine needs a password policy and login banner.")
N("bcrypt-regex", "a regex for bcrypt in code", 'HASH_RE = re.compile(r"^\\$2[aby]\\$\\d{2}\\$.{53}$")')
N("bare-pass-count", "a pass count in JSON", '{"pass": 12, "fail": 0}')
N("k8s-configmap", "a Kubernetes ConfigMap's data", "apiVersion: v1\nkind: ConfigMap\ndata:\n  LOG_LEVEL: debug\n  PORT: \"8080\"\n")
N("setter-plain", "a setter with a non-secret name", 'setTimeout("tick()", 100); setName("ana")')
N("xml-plain", "XML elements with ordinary names", "<user><name>ana</name><role>admin</role></user>")
N("wif-like-words", "a long base58-ish word", "Kubernetes1234567890abcdefghijkmnopqrstuvwxyzABCDEFGH")
N("telegram-like", "a time and ids", "12:30:45 job 123456789:done")
N("mailchimp-like", "a hex digest with -us in a file name", "d41d8cd98f00b204e9800998ecf8427e-usage.log")

# GATE-598: what follows a secret label that is code or a template, not a value (each structural test's cases)
N("type-rust-field", "a Rust field's type", "pub struct Login { pub token: String, pub password: Option<String> }")
N("type-ts", "TypeScript annotations", "interface Creds { password: string; apiKey: string }")
N("type-python-generic", "a Python generic annotation", "def f(secret: Optional[str] = None, token: bytes = b''): ...")
N("type-ref", "a Rust reference type", "fn check(password: &str) -> bool { password.len() > 8 }")
N("compare-eq", "a comparison", 'if token == expected { return; }\nif (password === "") {}')
N("compare-typeof", "a typeof check", 'if (typeof p.token !== "string" || typeof token === "string") {}')
N("placeholder-mustache", "a template placeholder", "password: {{ secrets.DB_PASSWORD }}")
N("placeholder-shell", "a shell variable", "export TOKEN=$GITHUB_TOKEN")
N("placeholder-windows", "a Windows variable", "set PASSWORD=%DB_PASSWORD%")
N("placeholder-format", "format placeholders", 'cur.execute("... password = %s", (pw,)); url = f"token={token}"')
N("placeholder-stars", "a masked display", "Password: ********")
N("placeholder-caps", "a variable named in caps", "api_key = OPENAI_API_KEY")
N("lookup-getenv", "os.getenv", 'password = os.getenv("DB_PASSWORD")')
N("lookup-rust-env", "env::var", 'let token = std::env::var("GITHUB_TOKEN")?;')
N("lookup-ruby-env", "ENV[...]", 'secret = ENV["SECRET_KEY_BASE"]')
N("lookup-config", "a config attribute", "password = config.db_password")
N("lookup-self", "an attribute", "token = self.token")
N("markup-after-label", "markup after a label", "<td>Password:</td><td><input name=pw></td>")
N("publishable-stripe", "a Stripe publishable key under a key label", "STRIPE_KEY=pk_live_" + r(AN, 40))

# Lines of this repo's own code that a first draft of the added rules masked (measured over the repo's tracked text):
# they must stay intact.
N("repo-peertoken", "a socket option named …PEERTOKEN (repo code)", "SOL_LOCAL, LOCAL_PEERTOKEN = 0, 0x006  # <sys/un.h>")
N("repo-last-pass", "a round of work named last_pass (repo code)", '"left_out": left_out, "last_pass": self.last_pass(),')
N("repo-merges-per-pass", "a constant named …_PER_PASS (repo code)", "pub const MERGES_PER_PASS: usize = 3;")
N("repo-match-arm", "Rust match arms on \"secret\" (repo code)", '"login_code" | "secret" => ("type", None),\n"fill_secret" => {\n"session" => out(runs.session(&s(&a[0]))),')
N("repo-seed-rows", "prose with 'seed' (repo docs)", "inserts the seed rows, runs the same steps and compares every return and every row")
N("repo-kc-pass", "a variable named kc_pass (repo code)", "kc_pass = secrets.token_hex(16)")
N("repo-probe-pw", "a JSON key probe.pw (repo fixture)", '"probe.pw": {')

# GATE-599's negatives: numbers that look like identifiers and are not, and data FERB needs to see (contact, technical)
N("pii-neg-verhoeff-fail", "a 4-4-4 number failing Verhoeff", f"ref {G_.aadhaar_bad()} attached")
N("pii-neg-plain12", "a plain 12-digit order number", "Order 402881293177 shipped")
N("pii-neg-phone-in", "an Indian phone number", "Call +91 98765 43210 today")
N("pii-neg-phone-uk", "a UK phone number", "Ring 020 7946 0958 or 07700 900123")
N("pii-neg-phone-us", "a US phone number", "(415) 555-0134 or 415-555-0134")
N("pii-neg-ts-ms", "a millisecond timestamp", "ts=1731654000000 seq=20261115103000")
N("pii-neg-ups", "a UPS tracking number", "Tracking 1Z999AA10123456784")
N("pii-neg-usps", "a USPS tracking number", "USPS 9400 1000 0000 0000 0000 00")
N("pii-neg-isbn", "an ISBN", "ISBN 978-3-16-148410-0")
N("pii-neg-invoice", "an invoice number", "Invoice INV-2026-004821, PO 4500123456")
N("pii-neg-amount", "amounts", "Total ₹1,23,456.00 ($1,482.50)")
N("pii-neg-coords", "coordinates", "at 12.9716, 77.5946 (Bengaluru)")
N("pii-neg-ifsc", "an IFSC (a branch, public)", "IFSC SBIN0001234, branch MG Road")
N("pii-neg-swift", "a SWIFT/BIC (a bank, public)", "SWIFT DEUTDEFF500")
N("pii-neg-upi", "a UPI id (a payment address, contact class)", "pay to ana.pereira@okaxis")
N("pii-neg-email", "an email address", "write to ana.pereira@example.com")
N("pii-neg-name-address", "a name and address", "Ana Pereira, 221B Baker Street, London NW1 6XE")
N("pii-neg-dob", "a date of birth", "born 14/03/1990, age 36")
N("pii-neg-imei", "an IMEI", "IMEI 35-209900-176148-1",
  known="the ported card rule: fifteen Luhn-valid digits in groups read as a payment card")
N("pii-neg-vin", "a VIN", "VIN 1HGCM82633A004352")
N("pii-neg-btc", "a crypto address (public)", "send to bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq")
N("pii-neg-postal", "a postal PIN code", "PIN code 560001, Bengaluru")
N("pii-neg-rgb", "an RGB triple (3-3-3 digits)", "color: rgb(170 145 239); fill: rgb(12 200 99)")
N("pii-neg-token-letter9", "a letter and nine digits in a token list", '{"tokens": ["S226905820", "A123456789x"]}')
N("pii-neg-sizes", "sizes", "16GB 512GB 1TB")
N("pii-neg-version-date", "versions and dates", "v2.10.3 released 2026-11-15, build 202611142038")
N("pii-neg-words", "label words in prose", "Bring your passport and your PAN card; the NHS will call. Run the tests.")
N("pii-neg-run-word", "a short label word in lower case", "run 12345678 tests then sin 123456789")

out = {"note": "NOT Python-recorded: the secret-shape corpus for ferb-sanitize's Rust-only shapes layer. Invented "
               "values only; made by make.py in this folder.",
       "positives": POS, "negatives": NEG}
(HERE / "corpus.json").write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
print(f"{len(POS)} positives, {len(NEG)} negatives")
