# mbtalerts

## Initial Setup

_Requires `openapi-generator` to be installed_

**TODO**: `/Users/jacob/Documents/Programs/mbta-api/apps/api_web/priv/static/swagger.json` in `openapi.json` should be
replaced with `https://api-v3.mbta.com/docs/swagger/swagger.json` once https://github.com/mbta/api/pull/871 is
merged.

```bash
openapi-generator generate -c src/mbta-client/openapi.json
```
