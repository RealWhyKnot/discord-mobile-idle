FROM python:3.13-alpine AS build

RUN apk add --no-cache binutils

COPY requirements.txt /tmp/requirements.txt
RUN python -m venv /venv \
    && /venv/bin/pip install --no-cache-dir --no-compile --disable-pip-version-check \
        --only-binary=aiohttp,multidict,yarl,frozenlist,propcache,curl_cffi,cffi,audioop-lts \
        -r /tmp/requirements.txt \
    && /venv/bin/python -OO -m compileall -q /venv/lib \
    && find /venv -type f -name '*.so*' -exec strip --strip-unneeded {} + \
    && rm -rf /venv/lib/python3.13/site-packages/pip \
        /venv/lib/python3.13/site-packages/pip-*.dist-info \
        /venv/bin/pip /venv/bin/pip3 /venv/bin/pip3.13

FROM python:3.13-alpine

ENV PATH=/venv/bin:$PATH \
    PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONOPTIMIZE=2

COPY --from=build /venv /venv

WORKDIR /app
COPY idlebot/ /app/idlebot/

ENTRYPOINT ["python", "-m", "idlebot"]
