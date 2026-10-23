"""Prepare private synthetic policy identities; never change production files."""
import ast
import hashlib
import importlib.util
from pathlib import Path
from unittest import mock


class PolicyOutputs(dict):
    """Keep the r15 layer for the older isolated transformation regressions."""
    before_access = None
    before_netflix = None
    before_openai = None


def serve_before_access(helper, directory, arguments):
    # Older policy tests assert the identity of their own fixed layer. The new
    # access suite and final runner/packaging tests exercise the complete chain.
    # No production switch permits this bypass.
    with mock.patch.object(helper, 'authorized_provider_access', side_effect=lambda role, data: data), mock.patch.object(helper, 'validated_netflix', side_effect=lambda role, data: data), mock.patch.object(helper, 'authorized_openai', side_effect=lambda role, data: data):
        return helper.serve(directory, arguments)


def serve_before_netflix(helper, directory, arguments):
    # Isolated r16 assertions retain their exact source identity. The new
    # Netflix suite and final signed runner tests exercise the complete chain.
    with mock.patch.object(helper, 'validated_netflix', side_effect=lambda role, data: data), mock.patch.object(helper, 'authorized_openai', side_effect=lambda role, data: data):
        return helper.serve(directory, arguments)


def serve_before_openai(helper, directory, arguments):
    # Recover the fixed Netflix layer only in isolated historical assertions.
    # Final source/runner tests and the OpenAI suite retain the complete chain.
    with mock.patch.object(helper, 'authorized_openai', side_effect=lambda role, data: data):
        return helper.serve(directory, arguments)


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def replace_once(content, before, after):
    if content.count(before) != 1:
        raise ValueError('synthetic fixture replacement must be unique')
    return content.replace(before, after, 1)


def inert_source(name, policy):
    source = ('#!/bin/sh\n# Synthetic inert fixture: ' + name + '\n').encode()
    if name in policy.SOURCES:
        source += b'check_bash(){\n:; }\n'
        if name in ('ip.sh', 'net.sh'):
            source += b'check_bash\n'
        source += b'fixture_report(){\n'
        if name == 'net.sh':
            source += policy.NET_OUTPUT
        source += policy.SOURCES[name]['original_guard'] + b'}\n'
    return source + swap_anchors(name) + dependency_anchors(name) + data_anchors(name) + loader_anchors(name) + ranking_anchors(name) + ip_score_anchors(name) + browser_anchors(name) + query_anchors(name) + access_anchors(name) + netflix_anchors(name) + openai_anchors(name) + ("printf '%s' '" + name + "' > \"$NQ_SOURCE_EXECUTED\"\n").encode()


def openai_anchors(name):
    if name != 'ip.sh':
        return b''
    # The access fixture already provides the unique save_json aggregate line.
    # No upstream authorization bytes are copied into this private fixture.
    return b'function OpenAITest(){\n: # Synthetic uncalled OpenAI function.\n}\n'


def netflix_anchors(name):
    if name != 'ip.sh':
        return b''
    policy = module('fixture_netflix_policy', Path(__file__).resolve().parents[1] / 'plugins/nodequality/native-netflix-policy.py')
    # The prior query layer already supplies FETCH. Only the original
    # classifier anchors are needed in the inert packaging source.
    extra = b''.join(before for before, _ in policy.REPLACEMENTS if before != policy.FETCH)
    return b"fixture_unused_netflix_classification(){\ncat <<'APSP_FIXTURE_NETFLIX'\n" + extra + b'APSP_FIXTURE_NETFLIX\n}\n'


def access_anchors(name):
    if name != 'ip.sh':
        return b''
    policy = module('fixture_access_policy', Path(__file__).resolve().parents[1] / 'plugins/nodequality/access-policy.py')
    functions = b''.join((key + '(){\n: # Synthetic uncalled provider function.\n}\n').encode()
                         for key in policy.FUNCTIONS)
    return functions + b"fixture_unused_access_metadata(){\ncat <<'APSP_FIXTURE_ACCESS'\n" + policy.JSON_ANCHOR + b'APSP_FIXTURE_ACCESS\n}\n'


def query_anchors(name):
    if name != 'ip.sh':
        return b''
    policy = module('fixture_query_policy', Path(__file__).resolve().parents[1] / 'plugins/nodequality/query-policy.py')
    return b"fixture_unused_queries(){\ncat <<'APSP_FIXTURE_QUERY'\n" + b''.join(a for a, _ in policy.REPLACEMENTS) + b'APSP_FIXTURE_QUERY\n}\n'


def undo_queries(role, patched):
    if role != 'ip.sh':
        return patched
    policy = module('fixture_undo_query', Path(__file__).resolve().parents[1] / 'plugins/nodequality/query-policy.py')
    if policy.HELPERS not in patched:
        return patched
    for before, after in reversed(policy.REPLACEMENTS):
        patched = replace_once(patched, after, before)
    return patched


def browser_anchors(name):
    if name not in ('ip.sh', 'net.sh'):
        return b''
    return b'fixture_unused_browser_identity(){\ngenerate_random_user_agent\n}\n'


def undo_browser(role, patched):
    patched = undo_queries(role, patched)
    if role not in ('ip.sh', 'net.sh'):
        return patched
    policy = module('fixture_undo_browser', Path(__file__).resolve().parents[1] / 'plugins/nodequality/native-browser-policy.py')
    for before, after in reversed(policy.REPLACEMENTS):
        patched = replace_once(patched, after, before)
    return patched


def ip_score_anchors(name):
    if name != 'ip.sh':
        return b''
    policy = module('fixture_ip_score_policy', Path(__file__).resolve().parents[1] / 'plugins/nodequality/ip-score-policy.py')
    return b"fixture_unused_ip_scores(){\ncat <<'APSP_FIXTURE_IP_SCORES'\n" + b''.join(a for a, _ in policy.REPLACEMENTS) + b'APSP_FIXTURE_IP_SCORES\n}\n'


def undo_ip_scores(role, patched):
    if role != 'ip.sh':
        return patched
    policy = module('fixture_undo_ip_scores', Path(__file__).resolve().parents[1] / 'plugins/nodequality/ip-score-policy.py')
    for before, after in reversed(policy.REPLACEMENTS):
        patched = replace_once(patched, after, before)
    return patched


def ranking_anchors(name):
    if name != 'hardware.sh':
        return b''
    policy = module('fixture_ranking_policy', Path(__file__).resolve().parents[1] / 'plugins/nodequality/ranking-policy.py')
    # These anchors stay inside an uncalled heredoc; no script runs here.
    return b"fixture_unused_ranking(){\ncat <<'APSP_FIXTURE_RANKING'\n" + b''.join(a for a, _ in policy.REPLACEMENTS) + b'APSP_FIXTURE_RANKING\n}\n'


def undo_ranking(role, patched):
    if role != 'hardware.sh':
        return patched
    policy = module('fixture_undo_ranking', Path(__file__).resolve().parents[1] / 'plugins/nodequality/ranking-policy.py')
    for before, after in reversed(policy.REPLACEMENTS):
        patched = replace_once(patched, after, before)
    return patched


def loader_anchors(name, existing=b''):
    if name != 'NodeQuality.sh':
        return b''
    policy = module('fixture_loader_policy', Path(__file__).resolve().parents[1] / 'plugins/nodequality/loader-policy.py')
    missing = [before for before, _ in policy.REPLACEMENTS if before not in existing]
    return b'fixture_unused_loaders(){\n' + b''.join(missing) + b'}\n'


def undo_loader(role, patched):
    if role != 'NodeQuality.sh':
        return patched
    policy = module('fixture_undo_loader', Path(__file__).resolve().parents[1] / 'plugins/nodequality/loader-policy.py')
    for before, after in reversed(policy.REPLACEMENTS):
        patched = replace_once(patched, after, before)
    return patched


def data_anchors(name):
    policy = module('fixture_data_policy', Path(__file__).resolve().parents[1] / 'plugins/nodequality/data-policy.py')
    if name not in policy.REQUESTS:
        return b''
    return b'fixture_unused_data(){\n' + b'\n'.join(policy.REQUESTS[name]) + b'\n}\n'


def undo_data(role, patched, contents):
    policy = module('fixture_undo_data', Path(__file__).resolve().parents[1] / 'plugins/nodequality/data-policy.py')
    for request, name in policy.REQUESTS.get(role, {}).items():
        patched = replace_once(patched, policy.data_command(contents[name]), request)
    return patched


def assignment(content, name, value):
    statements = [node for node in ast.parse(content).body if isinstance(node, ast.Assign)
                  and any(isinstance(target, ast.Name) and target.id == name for target in node.targets)]
    if len(statements) != 1:
        raise ValueError('fixture policy assignment must be unique')
    node = statements[0]
    lines = content.splitlines(keepends=True)
    return b''.join(lines[:node.lineno - 1]) + (name + ' = ' + repr(value) + '\n').encode() + b''.join(lines[node.end_lineno:])


def dependency_anchors(name):
    policy = module('fixture_dependency_policy', Path(__file__).resolve().parents[1] / 'plugins/nodequality/dependency-policy.py')
    if name == 'NodeQuality.sh':
        return b'fixture_unused_dependencies(){\n' + b''.join(a for a, _ in policy.ENTRY_REPLACEMENTS) + b'}\n'
    if name in policy.END_MARKERS:
        end = policy.END_MARKERS[name]
        suffix = b':\n}\n' if name == 'hardware.sh' else b')\n'
        return b'fixture_unused_dependencies(){\ninstall_dependencies(){\n:\n}\n' + end + suffix + policy.CHECK_CALL + b'}\n'
    return b''


def swap_anchors(name):
    policy = module('fixture_swap_policy', Path(__file__).resolve().parents[1] / 'plugins/nodequality/swap-policy.py')
    if name == 'NodeQuality.sh':
        return b'fixture_unused_entry_swaps(){\n' + b''.join(a for a, _ in policy.ENTRY_REPLACEMENTS) + b'}\n'
    if name == 'hardware.sh':
        return policy.HARDWARE_PREFIX + b':\ncleanup_local(){\n' + policy.SWAP_CLEANUP + b':\n}\n}\n'
    return b''


def undo_dependencies(role, patched, original):
    policy = module('fixture_undo_dependencies', Path(__file__).resolve().parents[1] / 'plugins/nodequality/dependency-policy.py')
    if role == 'NodeQuality.sh':
        for before, after in reversed(policy.ENTRY_REPLACEMENTS):
            patched = replace_once(patched, after, before)
        return patched
    start, stop = policy.installer_span(role, original)
    patched = replace_once(patched, policy.checks(role), original[start:stop])
    return replace_once(patched, policy.REQUIRED_CALL, policy.CHECK_CALL)


def prepare_policy(plugin, contents):
    """Caller owns a private copy of the plugin tree and all synthetic inputs."""
    path = Path(plugin) / 'report-policy.py'
    policy = module('synthetic_report_policy_input', path)
    content = path.read_bytes()
    original_helper_hash = hashlib.sha256(content).hexdigest().encode()
    outputs = PolicyOutputs()
    for role, spec in policy.SOURCES.items():
        canonical = contents[role]
        patched = policy.exact_replace(canonical, b'check_bash(){\n', policy.POLICY + b'check_bash(){\n')
        patched = policy.exact_replace(patched, spec['original_guard'], spec['patched_guard'])
        if role == 'net.sh':
            patched = policy.exact_replace(patched, policy.NET_OUTPUT, b'local report_link=""\n' + policy.NET_OUTPUT)
        content = replace_once(content, spec['source_sha256'].encode(), hashlib.sha256(canonical).hexdigest().encode())
        content = replace_once(content, spec['patched_sha256'].encode(), hashlib.sha256(patched).hexdigest().encode())
        outputs[role] = patched
    path.write_bytes(content)
    helper = Path(plugin) / 'native-source-helper.py'
    helper.write_bytes(replace_once(helper.read_bytes(), original_helper_hash, hashlib.sha256(content).hexdigest().encode()))
    swap_path = Path(plugin) / 'swap-policy.py'
    swap = module('synthetic_swap_policy_input', swap_path)
    content = swap_path.read_bytes()
    original_swap_hash = hashlib.sha256(content).hexdigest().encode()
    for role, spec in swap.SOURCES.items():
        canonical = outputs.get(role, contents[role])
        patched = swap.patch(role, canonical)
        content = replace_once(content, spec['source_sha256'].encode(), hashlib.sha256(canonical).hexdigest().encode())
        content = replace_once(content, spec['patched_sha256'].encode(), hashlib.sha256(patched).hexdigest().encode())
        outputs[role] = patched
    swap_path.write_bytes(content)
    helper.write_bytes(replace_once(helper.read_bytes(), original_swap_hash, hashlib.sha256(content).hexdigest().encode()))
    dependency_path = Path(plugin) / 'dependency-policy.py'
    dependency = module('synthetic_dependency_policy_input', dependency_path)
    content = dependency_path.read_bytes()
    original_hash = hashlib.sha256(content).hexdigest().encode()
    for role, spec in dependency.SOURCES.items():
        canonical = outputs.get(role, contents[role])
        patched = dependency.patch(role, canonical)
        content = replace_once(content, spec['source_sha256'].encode(), hashlib.sha256(canonical).hexdigest().encode())
        content = replace_once(content, spec['patched_sha256'].encode(), hashlib.sha256(patched).hexdigest().encode())
        outputs[role] = patched
    dependency_path.write_bytes(content)
    helper.write_bytes(replace_once(helper.read_bytes(), original_hash, hashlib.sha256(content).hexdigest().encode()))
    data_path = Path(plugin) / 'data-policy.py'
    data_policy = module('synthetic_data_policy_input', data_path)
    content = data_path.read_bytes()
    original_hash = hashlib.sha256(content).hexdigest().encode()
    identities = {name: {'sha256': hashlib.sha256(contents[name]).hexdigest(), 'size': len(contents[name])}
                  for name in data_policy.DATA}
    content = assignment(content, 'DATA', identities)
    for role, spec in data_policy.SOURCES.items():
        canonical = outputs[role]
        patched = data_policy.patch(role, canonical, {name: contents[name] for name in data_policy.REQUESTS[role].values()})
        content = replace_once(content, spec['source_sha256'].encode(), hashlib.sha256(canonical).hexdigest().encode())
        content = replace_once(content, spec['patched_sha256'].encode(), hashlib.sha256(patched).hexdigest().encode())
        outputs[role] = patched
    data_path.write_bytes(content)
    helper.write_bytes(replace_once(helper.read_bytes(), original_hash, hashlib.sha256(content).hexdigest().encode()))
    loader_path = Path(plugin) / 'loader-policy.py'
    loader = module('synthetic_loader_policy_input', loader_path)
    content = loader_path.read_bytes()
    original_hash = hashlib.sha256(content).hexdigest().encode()
    for role, spec in loader.SOURCES.items():
        canonical = outputs[role]
        patched = loader.patch(canonical)
        content = replace_once(content, spec['source_sha256'].encode(), hashlib.sha256(canonical).hexdigest().encode())
        content = replace_once(content, spec['patched_sha256'].encode(), hashlib.sha256(patched).hexdigest().encode())
        outputs[role] = patched
    loader_path.write_bytes(content)
    helper.write_bytes(replace_once(helper.read_bytes(), original_hash, hashlib.sha256(content).hexdigest().encode()))
    ranking_path = Path(plugin) / 'ranking-policy.py'
    ranking = module('synthetic_ranking_policy_input', ranking_path)
    content = ranking_path.read_bytes()
    original_hash = hashlib.sha256(content).hexdigest().encode()
    for role, spec in ranking.SOURCES.items():
        canonical = outputs[role]
        patched = ranking.patch(canonical)
        content = replace_once(content, spec['source_sha256'].encode(), hashlib.sha256(canonical).hexdigest().encode())
        content = replace_once(content, spec['patched_sha256'].encode(), hashlib.sha256(patched).hexdigest().encode())
        outputs[role] = patched
    ranking_path.write_bytes(content)
    helper.write_bytes(replace_once(helper.read_bytes(), original_hash, hashlib.sha256(content).hexdigest().encode()))
    score_path = Path(plugin) / 'ip-score-policy.py'
    scores = module('synthetic_ip_score_policy_input', score_path)
    content = score_path.read_bytes()
    original_hash = hashlib.sha256(content).hexdigest().encode()
    for role, spec in scores.SOURCES.items():
        canonical = outputs[role]
        patched = scores.patch(canonical)
        content = replace_once(content, spec['source_sha256'].encode(), hashlib.sha256(canonical).hexdigest().encode())
        content = replace_once(content, spec['patched_sha256'].encode(), hashlib.sha256(patched).hexdigest().encode())
        outputs[role] = patched
    score_path.write_bytes(content)
    helper.write_bytes(replace_once(helper.read_bytes(), original_hash, hashlib.sha256(content).hexdigest().encode()))
    browser_path = Path(plugin) / 'native-browser-policy.py'
    browser = module('synthetic_browser_policy_input', browser_path)
    content = browser_path.read_bytes()
    original_hash = hashlib.sha256(content).hexdigest().encode()
    for role, spec in browser.SOURCES.items():
        canonical = outputs[role]
        patched = browser.patch(canonical)
        content = replace_once(content, spec['source_sha256'].encode(), hashlib.sha256(canonical).hexdigest().encode())
        content = replace_once(content, spec['patched_sha256'].encode(), hashlib.sha256(patched).hexdigest().encode())
        outputs[role] = patched
    browser_path.write_bytes(content)
    helper.write_bytes(replace_once(helper.read_bytes(), original_hash, hashlib.sha256(content).hexdigest().encode()))
    query_path = Path(plugin) / 'query-policy.py'
    query = module('synthetic_query_policy_input', query_path)
    content = query_path.read_bytes()
    original_hash = hashlib.sha256(content).hexdigest().encode()
    for role, spec in query.SOURCES.items():
        canonical = outputs[role]
        patched = query.patch(canonical)
        content = replace_once(content, spec['source_sha256'].encode(), hashlib.sha256(canonical).hexdigest().encode())
        content = replace_once(content, spec['patched_sha256'].encode(), hashlib.sha256(patched).hexdigest().encode())
        outputs[role] = patched
    query_path.write_bytes(content)
    helper.write_bytes(replace_once(helper.read_bytes(), original_hash, hashlib.sha256(content).hexdigest().encode()))
    outputs.before_access = dict(outputs)
    access_path = Path(plugin) / 'access-policy.py'
    access = module('synthetic_access_policy_input', access_path)
    content = access_path.read_bytes()
    original_hash = hashlib.sha256(content).hexdigest().encode()
    for role, spec in access.SOURCES.items():
        canonical = outputs[role]
        patched = access.patch(canonical)
        content = replace_once(content, spec['source_sha256'].encode(), hashlib.sha256(canonical).hexdigest().encode())
        content = replace_once(content, spec['patched_sha256'].encode(), hashlib.sha256(patched).hexdigest().encode())
        outputs[role] = patched
    access_path.write_bytes(content)
    helper.write_bytes(replace_once(helper.read_bytes(), original_hash, hashlib.sha256(content).hexdigest().encode()))
    outputs.before_netflix = dict(outputs)
    netflix_path = Path(plugin) / 'native-netflix-policy.py'
    netflix = module('synthetic_netflix_policy_input', netflix_path)
    content = netflix_path.read_bytes()
    original_hash = hashlib.sha256(content).hexdigest().encode()
    for role, spec in netflix.SOURCES.items():
        canonical = outputs[role]
        patched = netflix.patch(canonical)
        content = replace_once(content, spec['source_sha256'].encode(), hashlib.sha256(canonical).hexdigest().encode())
        content = replace_once(content, spec['patched_sha256'].encode(), hashlib.sha256(patched).hexdigest().encode())
        outputs[role] = patched
    netflix_path.write_bytes(content)
    helper.write_bytes(replace_once(helper.read_bytes(), original_hash, hashlib.sha256(content).hexdigest().encode()))
    outputs.before_openai = dict(outputs)
    openai_path = Path(plugin) / 'openai-policy.py'
    openai = module('synthetic_openai_policy_input', openai_path)
    content = openai_path.read_bytes()
    identities = {}
    for role in openai.SOURCES:
        canonical = outputs[role]
        patched = openai.patch(canonical)
        identities[role] = {'source_sha256': hashlib.sha256(canonical).hexdigest(),
                            'patched_sha256': hashlib.sha256(patched).hexdigest()}
        outputs[role] = patched
    content = assignment(content, 'SOURCES', identities)
    openai_path.write_bytes(content)
    helper.write_bytes(assignment(helper.read_bytes(), 'OPENAI_POLICY_SHA256', hashlib.sha256(content).hexdigest()))
    return outputs
