#!/usr/bin/env python3
# SPDX-License-Identifier: LGPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# INFERCORE (SR57): record the reference KATs ONCE (their sha256 is pinned in tests/oracle.rs).
#
#   PYTHONPATH=<site> python3 kat/record.py <dir with tokenizer.json + model.onnx>
#
# Needs `tokenizers`, `onnxruntime`, `numpy` (pip). Writes, next to this script:
#   tokenizer_kat_{0,1}.txt  2000 strings → HF `tokenizers` ids (no truncation), one per line:
#                            <JSON string> TAB <ids space-separated>   (two files: < 200 KB each)
#   sentences.txt            the 100 oracle sentences, one JSON string per line
#   minilm_ort.f32           their all-MiniLM-L6-v2 vectors, 100 × 384 little-endian f32: HF
#                            tokenizers (truncation 256, as sentence-transformers) → onnxruntime
#                            (last_hidden_state) → attention-masked mean → x / max(‖x‖, 1e-12),
#                            in float32 as sentence-transformers' Pooling + Normalize compute it.
#   reader_kat.safetensors   a small file written by HF's own `safetensors` writer (F32, F16, BF16,
#                            I64; plus `__metadata__`), whose metadata carries, per float tensor,
#                            the f32 bit patterns numpy / ml_dtypes give for its stored values.
import json, os, random, sys
import numpy as np
import ml_dtypes
import safetensors.numpy
import onnxruntime as ort
from tokenizers import Tokenizer

HERE = os.path.dirname(os.path.abspath(__file__))
model_dir = sys.argv[1]

tok = Tokenizer.from_file(os.path.join(model_dir, 'tokenizer.json'))
tok.no_truncation()
tok.no_padding()
vocab = [w for w, _ in sorted(tok.get_vocab().items(), key=lambda kv: kv[1]) if w.isalpha() and not w.startswith('##')]

# ---- the 2000 tokenizer strings (deterministic) ----
rng = random.Random(20261005)
POOLS = {
    'latin_acc': 'àáâãäåāăąçćĉċčďđèéêëēĕėęěĝğġģĥħìíîïĩīĭįıĵķĺļľŀłñńņňŉòóôõöøōŏőœŕŗřśŝşšţťŧùúûüũūŭůűųŵýÿŷźżžÀÁÂÃÄÅÇÈÉÊËÌÍÎÏÑÒÓÔÕÖØÙÚÛÜÝßẞǅǈǋǲĲĳǄǇǊ',
    'combining': [chr(c) for c in list(range(0x300, 0x370)) + [0x1AB0, 0x1DC0, 0x20D0, 0x20E3, 0xFE20, 0x0483, 0x0591, 0x05B0, 0x064B, 0x0670, 0x093C, 0x094D, 0x0E31, 0x0E48, 0x1D165, 0x1D167]],
    'greek': 'αβγδεζηθικλμνξοπρσςτυφχψωΑΒΓΔΣΩάέήίόύώΐΰϊϋΆΈΉΊΌΎΏ',
    'cyrillic': 'абвгдеёжзийклмнопрстуфхцчшщъыьэюяАБВГДЕЁЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯїієґЇІЄҐ',
    'arabic': 'ابتثجحخدذرزسشصضطظعغفقكلمنهوي' + 'ًٌٍَُِّْ' + '،؛؟٠١٢٣' + '؜؀۝࣢',
    'hebrew': 'אבגדהוזחטיכלמנסעפצקרשת' + 'ְִַּׁ' + '־׳',
    'indic': 'कखगघचछजझटठडढणतथदधनपफबभमयरलवशषसह' + 'ािीुूेैोौंः्' + '।॥' + 'ਕਖਗ' + 'அஆஇ' + 'ःा',
    'thai': 'กขฃคฅฆงจฉชซฌญฎฏฐฑฒณดตถทธนบปผฝพฟภมยรลวศษสหฬอฮ' + 'ัิีึืุู่้๊๋' + '๏๚๛',
    'cjk': [chr(c) for c in [0x4E00, 0x4E2D, 0x6587, 0x5B57, 0x65E5, 0x672C, 0x8A9E, 0x9FFF, 0x3400, 0x4DBF, 0x20000, 0x2A6D6, 0x2A700, 0x2B740, 0x2B81D, 0x2B820, 0x2B8FF, 0x2B91F, 0x2B920, 0x2CEA1, 0xF900, 0xFAD9, 0x2F800, 0x2FA1D, 0x3007, 0x3005, 0x9FA5, 0x9FCC, 0x9FD5, 0x30000]],
    'kana_hangul': 'あいうえおかきくけこがぎぐげごぱぴぷアイウエオカキクケコガギグゲゴパピプーッャュョ゛゜' + '가각간갇갈한국어글대한민국ᄀᄁᆨㄱㄴㅏ',
    'emoji': ['😀', '😂', '🥲', '🫠', '🫡', '🧑‍💻', '👩🏽‍🚀', '❤️', '✨', '🇺🇸', '🇯🇵', '🏳️‍🌈', '👍🏿', '🦀', '🪿', '🫎', '©', '®', '™', '☃', '⌘', '⚡', '🔥', '⃣', '1️⃣', '🀄', '🃏', '🪐', '⭐', '\U0001FAE8'],
    'symbols': '$€£¥₿¢¤§¶†‡•…‰′″‹›«»“”‘’„‚–—―‐‑‒¡¿·×÷±∞≈≠≤≥∑∏√∫∂∆∇∈∉∩∪⊂⊃⊕⊗°℃℉№℗℠Ω℧ℵ←→↑↓↔⇒⇔■□▲△●○◆◇★☆♠♣♥♦♪♫☺☻⌂⌐¬¦¨¯´¸^`~|\\_{}[]<>@#%&*+=',
    'cjk_punct': '、。〃〈〉《》「」『』【】〔〕〖〗〘〙〚〛〜〝〞〟〰〽・｡｢｣､！＂＃％＆＇（）＊，－．／：；？＠［＼］＿｛｝～￥￦',
    'fullwidth': 'ＡＢＣａｂｃ０１２３ｱｲｳﾞﾟ',
    'compat': 'ﬁﬂﬀﬃﬄﬅﬆ²³¹¼½¾ªº™℡㎏㎞㍿①②③ⅠⅡⅢⅰⅱ㈱㊤',
    'controls': ['\x00', '\x01', '\x07', '\x08', '\x0b', '\x0c', '\x1b', '\x1f', '\x7f', '\x80', '\x85', '\x9f', '­', '​', '‌', '‍', '‎', '‏', '‪', '‮', '⁠', '⁦', '⁩', '﻿', '￹', '￻', '\U000E0001', '\U000E0041', '\U0001BCA0', '\U000110BD', '\U000110CD', '᠎', '࢐', '�'],
    'private': ['', '', '', '', '', '\U000F0000', '\U000F0001', '\U000FFFFD', '\U00100000', '\U0010FFFD'],
    'unassigned': ['͸', '΀', '׿', '⿿', '\U0001FFFF', '\U0002FFFF', '\U000E0080', '￿', '￾', '\U0010FFFF', 'ࡰ', '\U0001E030'],
    'spaces': [' ', '\t', '\n', '\r', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', '　', '\x1c', '\x1d', '\x1e', '\x1f', '\x85'],
    'casing': ['İ', 'ı', 'ẞ', 'ß', 'Σ', 'ΣΑΣ', 'ὈΔΥΣΣΕΎΣ', 'ǅ', 'ǈ', 'ᾈ', 'ﬀ', 'Ω', 'K', 'Å', 'ϴ', 'ẛ', 'ŉ', 'ǰ', 'ΐ', 'Ⅻ', 'Ⓐ', '𐐀', '𞤀', 'Ꭰ', 'ꭰ', 'Ᲊ', 'ᲀ'],
    'specials': ['[CLS]', '[SEP]', '[MASK]', '[PAD]', '[UNK]', '[mask]', '[MASK', 'MASK]', '[[MASK]]', '[CLS][SEP]', '[MASK]s', ' [MASK] '],
}
PUNCT = list('.,!?;:\'"()-/') + ['...', '--', "'s", "n't"]

def eng(n):
    out = []
    for _ in range(n):
        w = rng.choice(vocab)
        r = rng.random()
        if r < 0.15: w = w.capitalize()
        elif r < 0.2: w = w.upper()
        out.append(w)
        if rng.random() < 0.15: out.append(rng.choice(PUNCT))
    sep = rng.choice([' ', ' ', ' ', '', '  '])
    return sep.join(out)

def pick(pool, k):
    p = POOLS[pool]
    return ''.join(rng.choice(p) for _ in range(k))

def rand_cp():
    while True:
        c = rng.choice([rng.randrange(0x20, 0x2000), rng.randrange(0x2000, 0x10000), rng.randrange(0x10000, 0x30000), rng.randrange(0xE0000, 0x110000)])
        if not 0xD800 <= c <= 0xDFFF:
            return chr(c)

def mixed():
    parts = []
    for _ in range(rng.randrange(1, 8)):
        k = rng.random()
        if k < 0.35: parts.append(eng(rng.randrange(1, 6)))
        elif k < 0.85: parts.append(pick(rng.choice(list(POOLS)), rng.randrange(1, 6)))
        else: parts.append(''.join(rand_cp() for _ in range(rng.randrange(1, 5))))
        parts.append(rng.choice(['', ' ', ' ', rng.choice(POOLS['spaces'])]))
    return ''.join(parts)

strings = []
strings += ['', ' ', 'a', 'A', '!', '##', '##s', 'hello', '[CLS]', '[MASK]', 'x' * 100, 'x' * 101, 'é' * 100, 'é' * 101,
            'a' + '́' * 120, 'unaffable', 'Unbelievable!!!', "don't stop-believing", 'é', 'é', 'Ä' * 3,
            'ΟΔΥΣΣΕΥΣ', 'ΣΑΣ σας', 'İstanbul', 'Straße STRASSE', '東京都', '𠀀𪛖', '\U0002B820\U0002B91F\U0002B920',
            '한국어 텍스트', 'ｈｅｌｌｏ　ｗｏｒｌｄ', 'ﬁnancial ﬂow', 'a​b', 'a­b', 'a﻿b', 'tab\there', 'line\nbreak',
            'cr\r\nlf', '\x00null\x00', 'repl�acement', 'pua', 'emoji 🧑‍💻 ok', 'flags 🇺🇸🇯🇵',
            'https://example.com/a?b=c&d=e#f', 'user@example.com', 'C++ & C# > Java?', '3.14159 1,000,000 1e-5',
            '¿Qué tal? ¡Bien!', '«quotes» “curly” ‘single’', 'em—dash en–dash', 'Ω ≠ Ω', 'Å vs Å', '࢐࣢؜']
for i in range(300): strings.append(eng(rng.randrange(1, 30)))
for pool in POOLS:
    for i in range(40): strings.append(pick(pool, rng.randrange(1, 12)) + rng.choice(['', ' ', ' x']) + pick(pool, rng.randrange(0, 6)))
for i in range(150): strings.append(''.join(rand_cp() for _ in range(rng.randrange(1, 20))))
for i in range(60):
    base = rng.choice(vocab)
    strings.append(''.join(c + ''.join(rng.choice(POOLS['combining']) for _ in range(rng.randrange(0, 4))) for c in base))
for i in range(40): strings.append(rng.choice(vocab) * rng.randrange(10, 40))
while len(strings) < 2000: strings.append(mixed())
strings = strings[:2000]
assert len(set(strings)) > 1900

lines = []
for s in strings:
    ids = tok.encode(s).ids
    lines.append(json.dumps(s, ensure_ascii=True) + '\t' + ' '.join(map(str, ids)))
for part in range(2):
    with open(os.path.join(HERE, f'tokenizer_kat_{part}.txt'), 'w') as f:
        f.write('\n'.join(lines[part * 1000:(part + 1) * 1000]) + '\n')

# ---- the 100 oracle sentences ----
golden = json.load(open(os.path.join(HERE, '..', '..', '..', '..', '..', 'libs', 'gneiss_pal', 'tests', 'fixtures', 'minilm_golden.json')))
sents = [s['text'] for s in golden['sentences']]
sents += [
    'A man is playing a guitar on stage.', 'Someone performs music with a stringed instrument.',
    'The stock market fell sharply on Monday.', 'Shares dropped steeply at the start of the week.',
    'She is reading a book in the library.', 'He enjoys hiking in the mountains every summer.',
    'The weather today is sunny with a light breeze.', 'Heavy rain is expected tomorrow afternoon.',
    'How do I reset my password?', 'What is the procedure to change my login credentials?',
    'The kernel scheduler preempts long-running tasks.', 'Interrupts are masked while the spinlock is held.',
    'A neural network learns weights by gradient descent.', 'Backpropagation computes the gradient of the loss.',
    'The recipe calls for two cups of flour and an egg.', 'Preheat the oven to 180 degrees Celsius.',
    'Water boils at one hundred degrees at sea level.', 'The Pacific is the largest ocean on Earth.',
    'Photosynthesis converts light into chemical energy.', 'Mitochondria are the powerhouse of the cell.',
    'I lost my keys somewhere in the park.', 'My phone battery died during the meeting.',
    'The train to Boston departs at 7:45 AM from platform 3.', 'Flights were delayed because of the snowstorm.',
    'He scored the winning goal in the final minute.', 'The orchestra rehearsed Beethoven\'s Ninth Symphony.',
    'Please summarize the attached report by Friday.', 'The invoice total is $1,234.56 including tax.',
    'Le chat dort sur le canapé.', 'Der Hund spielt im Garten.', 'El niño come una manzana.', 'Il treno è in ritardo.',
    'Москва — столица России.', 'Η Αθήνα είναι η πρωτεύουσα της Ελλάδας.', '東京は日本の首都です。', '北京是中国的首都。',
    '서울은 대한민국의 수도입니다.', 'القاهرة هي عاصمة مصر.', 'नई दिल्ली भारत की राजधानी है।', 'ירושלים היא עיר עתיקה.',
    'Ça coûte 5 € — trop cher ?', 'naïve coöperation façade über straße', 'ÀÉÎÕÜ àéîõü ÇçÑñ', 'Zürich, Malmö, Kraków, Reykjavík',
    'OK', 'yes', 'No.', '?', '...', '😀🎉🚀', 'C++ templates and Rust traits', 'fn main() { println!("hi"); }',
    'SELECT * FROM users WHERE id = 42;', 'https://unaos.example/docs/embed?lang=en', 'e = mc^2 and a^2 + b^2 = c^2',
    'The [MASK] sat on the mat.', '[CLS] already special [SEP]', 'tab\tseparated\tvalues', 'line one\nline two',
    'ALL CAPS SENTENCE FOR EMPHASIS', 'mIxEd CaSe wOrDs eVeRyWhErE', 'repeated repeated repeated repeated words',
    'pneumonoultramicroscopicsilicovolcanoconiosis is a long word', 'Supercalifragilisticexpialidocious!',
    'The year 1969 saw the first Moon landing.', 'Call me at +1 (555) 010-9999 tomorrow.',
    'Vein recalls memories stored in the vault by meaning.', 'Lumen shows the settings dialog for the embedder.',
    'The quick brown fox jumps over the lazy dog. ' * 12,
    ' '.join(['Long sentences are truncated at two hundred and fifty six tokens by sentence-transformers'] * 25),
    'an', 'the', 'café', 'résumé', 'naïveté', 'über', 'jalapeño', 'piñata', 'smörgåsbord', 'crème brûlée',
]
assert len(sents) == 100, len(sents)
with open(os.path.join(HERE, 'sentences.txt'), 'w') as f:
    f.write('\n'.join(json.dumps(s, ensure_ascii=True) for s in sents) + '\n')

sess = ort.InferenceSession(os.path.join(model_dir, 'model.onnx'), providers=['CPUExecutionProvider'])
trunc = Tokenizer.from_file(os.path.join(model_dir, 'tokenizer.json'))
trunc.enable_truncation(256)
trunc.no_padding()
out = []
for s in sents:
    e = trunc.encode(s)
    ids = np.array([e.ids], dtype=np.int64)
    mask = np.ones_like(ids)
    h = sess.run(['last_hidden_state'], {'input_ids': ids, 'attention_mask': mask, 'token_type_ids': np.zeros_like(ids)})[0]
    m = mask[..., None].astype(np.float32)
    pooled = (h * m).sum(1) / np.clip(m.sum(1), 1e-9, None)
    v = pooled / np.maximum(np.linalg.norm(pooled, axis=1, keepdims=True), 1e-12)
    out.append(v[0].astype('<f4'))
np.stack(out).astype('<f4').tofile(os.path.join(HERE, 'minilm_ort.f32'))
print('recorded', len(strings), 'strings,', len(sents), 'sentences; max tokens', max(len(trunc.encode(s).ids) for s in sents))

# ---- the safetensors reader KAT ----
r = np.random.default_rng(5)
base = np.concatenate([np.array([0.0, -0.0, 1.0, -1.5, 65504.0, 1e-7, 6.1e-5, 3.0e38, -3.0e38, 0.1], np.float32), r.standard_normal(54).astype(np.float32)])
tensors = {
    'f32': base.reshape(8, 8),
    'f16': base.astype(np.float16).reshape(4, 16),
    'bf16': base.astype(ml_dtypes.bfloat16).reshape(2, 4, 8),
    'ids': np.arange(12, dtype=np.int64).reshape(3, 4),
    'scalar': np.array(2.5, np.float32),
}
meta = {'format': 'pt'}
for k in ('f32', 'f16', 'bf16', 'scalar'):
    meta['expect.' + k] = ' '.join('%08x' % b for b in tensors[k].astype(np.float32).reshape(-1).view(np.uint32))
safetensors.numpy.save_file(tensors, os.path.join(HERE, 'reader_kat.safetensors'), metadata=meta)
