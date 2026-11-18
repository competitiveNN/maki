local numbered_list = require("numbered_list")
local constants = require("constants")
local searxng = require("searxng")
local NO_RESULTS_MSG = constants.NO_RESULTS_MSG
local NOT_AN_OBJECT_MSG = constants.NOT_AN_OBJECT_MSG

local REQUEST_TIMEOUT_SECS = 25

local function fetch(url, req_opts)
  local resp, err = maki.net.request(url, req_opts)
  if not resp then
    return nil, tostring(err)
  end
  if resp.status < 200 or resp.status >= 300 then
    return nil, "HTTP " .. tostring(resp.status) .. ": " .. resp.body:sub(1, 200)
  end
  return resp
end

local function trim(s)
  return (tostring(s or ""):match("^%s*(.-)%s*$"))
end

local function api_key(env)
  local key = trim(maki.uv.os_getenv(env))
  return key ~= "" and key or nil
end

local function numbered(items, n)
  return numbered_list(items, n)
end

local providers = {
  exa = {
    label = "Exa AI",
    endpoint = "https://mcp.exa.ai/mcp",
    env = "EXA_API_KEY",
    auth_header = "x-api-key",
    tool = "web_search_exa",
    arguments = function(query, num_results)
      return {
        query = query,
        numResults = num_results,
        type = "auto",
        livecrawl = "fallback",
      }
    end,
  },
  parallel = {
    label = "Parallel AI",
    endpoint = "https://search.parallel.ai/mcp",
    env = "PARALLEL_API_KEY",
    auth_header = "x-api-key",
    auth_prefix = "",
    tool = "web_search",
    arguments = function(query, num_results)
      return {
        objective = query,
        search_queries = { query },
      }
    end,
    key_required = false,
  },
  tinyfish = {
    label = "TinyFish",
    env = "TINYFISH_API_KEY",
    transport = "rest",
    search = function(query, num_results)
      local key = api_key("TINYFISH_API_KEY")
      if not key then
        return nil, "TinyFish search needs TINYFISH_API_KEY to be set"
      end
      local url = "https://api.search.tinyfish.ai?query=" .. searxng.encode_param(query)
      if num_results then
        url = url .. "&num_results=" .. tostring(num_results)
      end
      local resp, err = fetch(url, {
        method = "GET",
        headers = { ["Accept"] = "application/json", ["X-API-Key"] = key },
        timeout = 25,
      })
      if not resp then return nil, err end
      local data, dec_err = maki.json.decode(resp.body, { luanil = { object = true, array = true } })
      if type(data) ~= "table" or type(data.results) ~= "table" then
        return nil, "TinyFish returned no results array" .. (dec_err and (": " .. tostring(dec_err)) or "")
      end
      local items = {}
      for _, r in ipairs(data.results) do
        if type(r) == "table" and type(r.url) == "string" then
          table.insert(items, {
            title = r.title or r.site_name or r.url,
            url = r.url,
            snippet = r.snippet or "",
          })
        end
      end
      if #items == 0 then return NO_RESULTS_MSG end
      return numbered_list(items, num_results)
    end,
  },
  jina = {
    label = "Jina",
    env = "JINA_API_KEY",
    transport = "rest",
    search = function(query, num_results)
      local key = api_key("JINA_API_KEY")
      if not key then
        return nil, "Jina search needs JINA_API_KEY to be set"
      end
      local url = "https://s.jina.ai/" .. searxng.encode_param(query)
      local resp, err = fetch(url .. "?count=" .. tostring(num_results or 5), {
        method = "GET",
        headers = {
          ["Accept"] = "application/json",
          ["Authorization"] = "Bearer " .. key,
          ["X-Respond-With"] = "no-content",
          ["X-Retain-Images"] = "none",
        },
        timeout = 25,
      })
      if not resp then return nil, err end
      local data, dec_err = maki.json.decode(resp.body, { luanil = { object = true, array = true } })
      local results = data
      if type(data) == "table" and not (type(data) == "table" and data[1]) then
        if type(data.data) == "table" then results = data.data end
      end
      if type(results) ~= "table" then
        return nil, "Jina returned invalid response" .. (dec_err and (": " .. tostring(dec_err)) or "")
      end
      local items = {}
      for _, r in ipairs(results) do
        if type(r) == "table" and type(r.url) == "string" then
          table.insert(items, {
            title = r.title or r.url,
            url = r.url,
            snippet = r.description or r.content or "",
          })
        end
      end
      if #items == 0 then return NO_RESULTS_MSG end
      return numbered_list(items, num_results)
    end,
  },
  tavily = {
    label = "Tavily",
    env = "TAVILY_API_KEY",
    transport = "rest",
    search = function(query, num_results)
      local key = api_key("TAVILY_API_KEY")
      if not key then
        return nil, "Tavily search needs TAVILY_API_KEY to be set"
      end
      local body, enc_err = maki.json.encode({
        query = query,
        max_results = num_results or 5,
        search_depth = "basic",
        include_answer = "advanced",
        include_raw_content = false,
      })
      if not body then return nil, "failed to encode request: " .. tostring(enc_err) end
      local resp, err = fetch("https://api.tavily.com/search", {
        method = "POST",
        headers = {
          ["Content-Type"] = "application/json",
          ["Authorization"] = "Bearer " .. key,
        },
        body = body,
        timeout = 25,
      })
      if not resp then return nil, err end
      local data, dec_err = maki.json.decode(resp.body, { luanil = { object = true, array = true } })
      if type(data) ~= "table" then
        return nil, "Tavily returned invalid JSON: " .. (dec_err or NOT_AN_OBJECT_MSG)
      end
      local items = {}
      if type(data.results) == "table" then
        for _, r in ipairs(data.results) do
          if type(r) == "table" and type(r.url) == "string" then
            table.insert(items, {
              title = r.title or r.url,
              url = r.url,
              snippet = r.content or "",
              published = r.published_date,
            })
          end
        end
      end
      local text = numbered_list(items, num_results)
      if type(data.answer) == "string" and data.answer ~= "" then
        text = data.answer .. "\n\n" .. text
      end
      return text
    end,
  },
  firecrawl = {
    label = "Firecrawl",
    env = "FIRECRAWL_API_KEY",
    transport = "rest",
    search = function(query, num_results)
      local key = api_key("FIRECRAWL_API_KEY")
      local base = trim(maki.uv.os_getenv("FIRECRAWL_BASE_URL") or maki.uv.os_getenv("FIRECRAWL_API_URL"))
      if base == "" then base = "https://api.firecrawl.dev/v2" end
      base = base:gsub("/+$", "")
      if not base:match("/v[12]$") then base = base .. "/v2" end
      local body, enc_err = maki.json.encode({
        query = query,
        limit = num_results or 10,
        sources = { { type = "web" } },
      })
      if not body then return nil, "failed to encode request: " .. tostring(enc_err) end
      local headers = { ["Content-Type"] = "application/json" }
      if key then headers["Authorization"] = "Bearer " .. key end
      local resp, err = fetch(base .. "/search", {
        method = "POST",
        headers = headers,
        body = body,
        timeout = 25,
      })
      if not resp then return nil, err end
      local data, dec_err = maki.json.decode(resp.body, { luanil = { object = true, array = true } })
      if type(data) ~= "table" then
        return nil, "Firecrawl returned invalid JSON: " .. (dec_err or NOT_AN_OBJECT_MSG)
      end
      local raw = data.data
      local results = {}
      if type(raw) == "table" and type(raw.web) == "table" then results = raw.web
      elseif type(raw) == "table" and raw[1] then results = raw
      elseif type(data.results) == "table" then results = data.results
      end
      local items = {}
      for _, r in ipairs(results) do
        if type(r) == "table" and type(r.url) == "string" then
          table.insert(items, {
            title = r.title or r.url,
            url = r.url,
            snippet = r.description or r.snippet or r.markdown or "",
          })
        end
      end
      if #items == 0 then return NO_RESULTS_MSG end
      return numbered_list(items, num_results)
    end,
  },
  searxng = {
    label = "SearXNG",
    transport = "get",
    env = "SEARXNG_BASE_URL",
  },
  youcom = {
    label = "You.com",
    endpoint = "https://api.you.com/mcp",
    env = "YDC_API_KEY",
    auth_header = "Authorization",
    auth_prefix = "Bearer ",
    tool = "you-search",
    arguments = function(query, num_results)
      return {
        query = query,
        count = num_results,
      }
    end,
    keyless_suffix = "?profile=free",
  },
  kagi = {
    label = "Kagi",
    endpoint = "https://mcp.kagi.com/mcp",
    key_required = true,
    env = "KAGI_API_KEY",
    auth_header = "Authorization",
    auth_prefix = "Bearer ",
    tool = "kagi_search_fetch",
    arguments = function(query, num_results)
      return {
        query = query,
        limit = num_results,
      }
    end,
    format = function(text)
      local data = maki.json.decode(text)
      if type(data) ~= "table" then return text end
      local KAGI_CATEGORIES = { "search", "news", "video", "podcast", "image" }
      local results = {}
      for _, category in ipairs(KAGI_CATEGORIES) do
        if type(data[category]) == "table" then
          for _, result in ipairs(data[category]) do
            table.insert(results, result)
          end
        end
      end
      if #results == 0 and next(data) ~= nil then
        return text
      end
      return numbered_list(results, #results)
    end,
  },
}

return providers