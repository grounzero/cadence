# SPDX-License-Identifier: GPL-3.0-or-later
"""Production settings layered over the pinned official OpenBench server."""

from pathlib import Path
import os

import OpenSite.settings as upstream_settings
from OpenSite.settings import *  # noqa: F403


def required_path(name: str) -> Path:
    value = os.environ.get(name)
    if not value:
        raise RuntimeError(f"{name} is required")
    return Path(value)


def comma_list(name: str) -> list[str]:
    return [item.strip() for item in os.environ.get(name, "").split(",") if item.strip()]


SECRET_KEY = required_path("OPENBENCH_SECRET_KEY_FILE").read_text(encoding="utf-8").strip()
if not SECRET_KEY:
    raise RuntimeError("OPENBENCH_SECRET_KEY_FILE is empty")

DEBUG = False
ALLOWED_HOSTS = comma_list("OPENBENCH_ALLOWED_HOSTS")
if not ALLOWED_HOSTS:
    raise RuntimeError("OPENBENCH_ALLOWED_HOSTS must name at least one host")

DATABASES = {
    "default": {
        "ENGINE": "django.db.backends.sqlite3",
        "NAME": str(required_path("OPENBENCH_DATABASE")),
    }
}
MEDIA_ROOT = str(required_path("OPENBENCH_MEDIA_ROOT"))
STATIC_ROOT = str(required_path("OPENBENCH_STATIC_ROOT"))

# OpenBench currently imports MEDIA_ROOT directly from OpenSite.settings in
# its upload and network paths. Mirror every production override into that
# module so direct imports cannot bypass the external state or security values.
upstream_settings.SECRET_KEY = SECRET_KEY
upstream_settings.DEBUG = DEBUG
upstream_settings.ALLOWED_HOSTS = ALLOWED_HOSTS
upstream_settings.DATABASES = DATABASES
upstream_settings.MEDIA_ROOT = MEDIA_ROOT
upstream_settings.STATIC_ROOT = STATIC_ROOT

csrf_origins = comma_list("OPENBENCH_CSRF_TRUSTED_ORIGINS")
if csrf_origins:
    CSRF_TRUSTED_ORIGINS = csrf_origins

if os.environ.get("OPENBENCH_BEHIND_HTTPS_PROXY") == "1":
    SECURE_PROXY_SSL_HEADER = ("HTTP_X_FORWARDED_PROTO", "https")
    SESSION_COOKIE_SECURE = True
    CSRF_COOKIE_SECURE = True
