use phf::phf_map;

#[derive(Debug, Clone, Copy)]
pub struct WordInfo {
    pub value: f64,
    pub is_multiplier: bool,
}

pub static WORD_VALUES: phf::Map<&'static str, WordInfo> = phf_map! {
    "k" => WordInfo { value: 1_000.0, is_multiplier: true },
    "к" => WordInfo { value: 1_000.0, is_multiplier: true },
    "kk" => WordInfo { value: 1_000_000.0, is_multiplier: true },
    "кк" => WordInfo { value: 1_000_000.0, is_multiplier: true },
    "m" => WordInfo { value: 1_000_000.0, is_multiplier: true },
    "м" => WordInfo { value: 1_000_000.0, is_multiplier: true },
    "mln" => WordInfo { value: 1_000_000.0, is_multiplier: true },
    "млн" => WordInfo { value: 1_000_000.0, is_multiplier: true },
    "b" => WordInfo { value: 1_000_000_000.0, is_multiplier: true },
    "б" => WordInfo { value: 1_000_000_000.0, is_multiplier: true },
    "bn" => WordInfo { value: 1_000_000_000.0, is_multiplier: true },
    "млрд" => WordInfo { value: 1_000_000_000.0, is_multiplier: true },
    "t" => WordInfo { value: 1_000_000_000_000.0, is_multiplier: true },
    "т" => WordInfo { value: 1_000_000_000_000.0, is_multiplier: true },
    "trln" => WordInfo { value: 1_000_000_000_000.0, is_multiplier: true },
    "трлн" => WordInfo { value: 1_000_000_000_000.0, is_multiplier: true },
    "тыс" => WordInfo { value: 1_000.0, is_multiplier: true },
    "тысяч" => WordInfo { value: 1_000.0, is_multiplier: true },
    "тысяча" => WordInfo { value: 1_000.0, is_multiplier: true },
    "thousand" => WordInfo { value: 1_000.0, is_multiplier: true },
    "million" => WordInfo { value: 1_000_000.0, is_multiplier: true },
    "billion" => WordInfo { value: 1_000_000_000.0, is_multiplier: true },
    "ноль" => WordInfo { value: 0.0, is_multiplier: false },
    "нуль" => WordInfo { value: 0.0, is_multiplier: false },
    "zero" => WordInfo { value: 0.0, is_multiplier: false },
};
