-- Setting Surge parameters in real units.
--
-- Every parameter is normalised 0..1 with an undocumented range, so values are
-- reached by searching against the plugin's own formatted readout rather than
-- by assuming a mapping. Units change with magnitude - milliseconds become
-- seconds, hertz become kilohertz - so the readout is normalised before it is
-- compared, otherwise the search converges on the wrong side of a unit change.

local M = {}

function M.parse(s)
  s = tostring(s)
  if s:find("inf") then
    return s:find("-") and -1e9 or 1e9
  end
  local num = s:match("(-?%d+%.?%d*)")
  if not num then return nil end
  local v = tonumber(num)
  if not v then return nil end
  if s:find("kHz") then
    v = v * 1000.0
  elseif s:find("%f[%a]s%f[%A]") and not s:find("ms") and not s:find("semi")
         and not s:find("cent") then
    v = v * 1000.0        -- seconds shown instead of milliseconds
  end
  return v
end

-- Reach `target` (in whatever unit the readout uses) on a monotonic parameter.
function M.setnum(tr, fx, p, target)
  local lo, hi = 0.0, 1.0
  for _ = 1, 34 do
    local mid = (lo + hi) / 2
    reaper.TrackFX_SetParamNormalized(tr, fx, p, mid)
    local _, s = reaper.TrackFX_GetFormattedParamValue(tr, fx, p, "")
    local v = M.parse(s)
    if v == nil then return false end
    if v < target then lo = mid else hi = mid end
  end
  reaper.TrackFX_SetParamNormalized(tr, fx, p, (lo + hi) / 2)
  return true
end

-- Enumerations have no ordering worth searching, so sweep and take the first
-- reading whose label contains `label`.
function M.setenum(tr, fx, p, label)
  local want = tostring(label):lower()
  for k = 0, 400 do
    local n = k / 400.0
    reaper.TrackFX_SetParamNormalized(tr, fx, p, n)
    local _, s = reaper.TrackFX_GetFormattedParamValue(tr, fx, p, "")
    if tostring(s):lower():find(want, 1, true) then return true end
  end
  return false
end

function M.shown(tr, fx, p)
  local _, s = reaper.TrackFX_GetFormattedParamValue(tr, fx, p, "")
  return tostring(s)
end

return M
