pub struct BeepNote {
    pub start_ms: u32,
    pub duration_ms: u16,
    pub frequency: u16,
}

pub static NOTES: &[BeepNote] = &[
    BeepNote {
        start_ms: 0,
        duration_ms: 180,
        frequency: 880,
    },
    BeepNote {
        start_ms: 361,
        duration_ms: 173,
        frequency: 880,
    },
    BeepNote {
        start_ms: 534,
        duration_ms: 180,
        frequency: 988,
    },
    BeepNote {
        start_ms: 899,
        duration_ms: 180,
        frequency: 1047,
    },
    BeepNote {
        start_ms: 1079,
        duration_ms: 180,
        frequency: 988,
    },
    BeepNote {
        start_ms: 1264,
        duration_ms: 173,
        frequency: 880,
    },
    BeepNote {
        start_ms: 1438,
        duration_ms: 180,
        frequency: 740,
    },
    BeepNote {
        start_ms: 1618,
        duration_ms: 180,
        frequency: 740,
    },
    BeepNote {
        start_ms: 1797,
        duration_ms: 180,
        frequency: 740,
    },
    BeepNote {
        start_ms: 1978,
        duration_ms: 180,
        frequency: 740,
    },
    BeepNote {
        start_ms: 2158,
        duration_ms: 20,
        frequency: 587,
    },
    BeepNote {
        start_ms: 2158,
        duration_ms: 180,
        frequency: 988,
    },
    BeepNote {
        start_ms: 2343,
        duration_ms: 81,
        frequency: 740,
    },
    BeepNote {
        start_ms: 2423,
        duration_ms: 92,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 2516,
        duration_ms: 90,
        frequency: 587,
    },
    BeepNote {
        start_ms: 2606,
        duration_ms: 90,
        frequency: 740,
    },
    BeepNote {
        start_ms: 2696,
        duration_ms: 90,
        frequency: 880,
    },
    BeepNote {
        start_ms: 2786,
        duration_ms: 90,
        frequency: 740,
    },
    BeepNote {
        start_ms: 2876,
        duration_ms: 90,
        frequency: 659,
    },
    BeepNote {
        start_ms: 2966,
        duration_ms: 90,
        frequency: 988,
    },
    BeepNote {
        start_ms: 3056,
        duration_ms: 180,
        frequency: 659,
    },
    BeepNote {
        start_ms: 3236,
        duration_ms: 180,
        frequency: 659,
    },
    BeepNote {
        start_ms: 3421,
        duration_ms: 180,
        frequency: 659,
    },
    BeepNote {
        start_ms: 3629,
        duration_ms: 20,
        frequency: 880,
    },
    BeepNote {
        start_ms: 3629,
        duration_ms: 180,
        frequency: 988,
    },
    BeepNote {
        start_ms: 3809,
        duration_ms: 90,
        frequency: 659,
    },
    BeepNote {
        start_ms: 3899,
        duration_ms: 90,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 3989,
        duration_ms: 93,
        frequency: 659,
    },
    BeepNote {
        start_ms: 4082,
        duration_ms: 92,
        frequency: 740,
    },
    BeepNote {
        start_ms: 4174,
        duration_ms: 81,
        frequency: 880,
    },
    BeepNote {
        start_ms: 4255,
        duration_ms: 92,
        frequency: 740,
    },
    BeepNote {
        start_ms: 4348,
        duration_ms: 180,
        frequency: 880,
    },
    BeepNote {
        start_ms: 4528,
        duration_ms: 180,
        frequency: 740,
    },
    BeepNote {
        start_ms: 4708,
        duration_ms: 180,
        frequency: 740,
    },
    BeepNote {
        start_ms: 4887,
        duration_ms: 180,
        frequency: 740,
    },
    BeepNote {
        start_ms: 5068,
        duration_ms: 20,
        frequency: 587,
    },
    BeepNote {
        start_ms: 5068,
        duration_ms: 180,
        frequency: 988,
    },
    BeepNote {
        start_ms: 5252,
        duration_ms: 81,
        frequency: 740,
    },
    BeepNote {
        start_ms: 5333,
        duration_ms: 93,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 5426,
        duration_ms: 90,
        frequency: 587,
    },
    BeepNote {
        start_ms: 5516,
        duration_ms: 90,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 5606,
        duration_ms: 90,
        frequency: 740,
    },
    BeepNote {
        start_ms: 5696,
        duration_ms: 90,
        frequency: 880,
    },
    BeepNote {
        start_ms: 5786,
        duration_ms: 180,
        frequency: 659,
    },
    BeepNote {
        start_ms: 5966,
        duration_ms: 180,
        frequency: 659,
    },
    BeepNote {
        start_ms: 6146,
        duration_ms: 180,
        frequency: 659,
    },
    BeepNote {
        start_ms: 6354,
        duration_ms: 150,
        frequency: 831,
    },
    BeepNote {
        start_ms: 6504,
        duration_ms: 91,
        frequency: 988,
    },
    BeepNote {
        start_ms: 6595,
        duration_ms: 90,
        frequency: 880,
    },
    BeepNote {
        start_ms: 6685,
        duration_ms: 180,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 6865,
        duration_ms: 127,
        frequency: 1245,
    },
    BeepNote {
        start_ms: 6992,
        duration_ms: 92,
        frequency: 988,
    },
    BeepNote {
        start_ms: 7084,
        duration_ms: 81,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 7165,
        duration_ms: 92,
        frequency: 1245,
    },
    BeepNote {
        start_ms: 7258,
        duration_ms: 92,
        frequency: 208,
    },
    BeepNote {
        start_ms: 7350,
        duration_ms: 93,
        frequency: 988,
    },
    BeepNote {
        start_ms: 7443,
        duration_ms: 81,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 7523,
        duration_ms: 92,
        frequency: 1661,
    },
    BeepNote {
        start_ms: 7616,
        duration_ms: 20,
        frequency: 208,
    },
    BeepNote {
        start_ms: 7616,
        duration_ms: 90,
        frequency: 988,
    },
    BeepNote {
        start_ms: 7706,
        duration_ms: 90,
        frequency: 1245,
    },
    BeepNote {
        start_ms: 7796,
        duration_ms: 90,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 7886,
        duration_ms: 90,
        frequency: 988,
    },
    BeepNote {
        start_ms: 7976,
        duration_ms: 20,
        frequency: 165,
    },
    BeepNote {
        start_ms: 7976,
        duration_ms: 180,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 8246,
        duration_ms: 90,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 8336,
        duration_ms: 92,
        frequency: 82,
    },
    BeepNote {
        start_ms: 8428,
        duration_ms: 180,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 8613,
        duration_ms: 81,
        frequency: 831,
    },
    BeepNote {
        start_ms: 8694,
        duration_ms: 20,
        frequency: 185,
    },
    BeepNote {
        start_ms: 8694,
        duration_ms: 180,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 8875,
        duration_ms: 90,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 8965,
        duration_ms: 90,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 9055,
        duration_ms: 20,
        frequency: 92,
    },
    BeepNote {
        start_ms: 9055,
        duration_ms: 124,
        frequency: 932,
    },
    BeepNote {
        start_ms: 9179,
        duration_ms: 90,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 9269,
        duration_ms: 90,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 9359,
        duration_ms: 90,
        frequency: 932,
    },
    BeepNote {
        start_ms: 9449,
        duration_ms: 20,
        frequency: 247,
    },
    BeepNote {
        start_ms: 9449,
        duration_ms: 93,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 9542,
        duration_ms: 93,
        frequency: 1245,
    },
    BeepNote {
        start_ms: 9634,
        duration_ms: 173,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 9808,
        duration_ms: 20,
        frequency: 233,
    },
    BeepNote {
        start_ms: 9808,
        duration_ms: 90,
        frequency: 1245,
    },
    BeepNote {
        start_ms: 9898,
        duration_ms: 90,
        frequency: 1661,
    },
    BeepNote {
        start_ms: 9988,
        duration_ms: 20,
        frequency: 233,
    },
    BeepNote {
        start_ms: 9988,
        duration_ms: 90,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 10078,
        duration_ms: 90,
        frequency: 1245,
    },
    BeepNote {
        start_ms: 10168,
        duration_ms: 92,
        frequency: 208,
    },
    BeepNote {
        start_ms: 10260,
        duration_ms: 93,
        frequency: 988,
    },
    BeepNote {
        start_ms: 10353,
        duration_ms: 81,
        frequency: 1245,
    },
    BeepNote {
        start_ms: 10433,
        duration_ms: 92,
        frequency: 1661,
    },
    BeepNote {
        start_ms: 10526,
        duration_ms: 20,
        frequency: 208,
    },
    BeepNote {
        start_ms: 10526,
        duration_ms: 90,
        frequency: 988,
    },
    BeepNote {
        start_ms: 10616,
        duration_ms: 90,
        frequency: 1245,
    },
    BeepNote {
        start_ms: 10706,
        duration_ms: 90,
        frequency: 831,
    },
    BeepNote {
        start_ms: 10796,
        duration_ms: 90,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 10886,
        duration_ms: 20,
        frequency: 165,
    },
    BeepNote {
        start_ms: 10886,
        duration_ms: 180,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 11066,
        duration_ms: 20,
        frequency: 165,
    },
    BeepNote {
        start_ms: 11066,
        duration_ms: 90,
        frequency: 988,
    },
    BeepNote {
        start_ms: 11156,
        duration_ms: 90,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 11246,
        duration_ms: 20,
        frequency: 165,
    },
    BeepNote {
        start_ms: 11246,
        duration_ms: 92,
        frequency: 1661,
    },
    BeepNote {
        start_ms: 11338,
        duration_ms: 93,
        frequency: 1245,
    },
    BeepNote {
        start_ms: 11431,
        duration_ms: 92,
        frequency: 831,
    },
    BeepNote {
        start_ms: 11523,
        duration_ms: 81,
        frequency: 1661,
    },
    BeepNote {
        start_ms: 11604,
        duration_ms: 20,
        frequency: 185,
    },
    BeepNote {
        start_ms: 11604,
        duration_ms: 91,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 11695,
        duration_ms: 90,
        frequency: 932,
    },
    BeepNote {
        start_ms: 11785,
        duration_ms: 20,
        frequency: 185,
    },
    BeepNote {
        start_ms: 11785,
        duration_ms: 90,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 11875,
        duration_ms: 124,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 11999,
        duration_ms: 20,
        frequency: 185,
    },
    BeepNote {
        start_ms: 11999,
        duration_ms: 92,
        frequency: 1865,
    },
    BeepNote {
        start_ms: 12092,
        duration_ms: 93,
        frequency: 2217,
    },
    BeepNote {
        start_ms: 12184,
        duration_ms: 81,
        frequency: 370,
    },
    BeepNote {
        start_ms: 12265,
        duration_ms: 92,
        frequency: 1865,
    },
    BeepNote {
        start_ms: 12358,
        duration_ms: 20,
        frequency: 247,
    },
    BeepNote {
        start_ms: 12358,
        duration_ms: 90,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 12448,
        duration_ms: 90,
        frequency: 1976,
    },
    BeepNote {
        start_ms: 12538,
        duration_ms: 20,
        frequency: 247,
    },
    BeepNote {
        start_ms: 12538,
        duration_ms: 90,
        frequency: 1245,
    },
    BeepNote {
        start_ms: 12628,
        duration_ms: 90,
        frequency: 2489,
    },
    BeepNote {
        start_ms: 12718,
        duration_ms: 20,
        frequency: 247,
    },
    BeepNote {
        start_ms: 12718,
        duration_ms: 90,
        frequency: 1976,
    },
    BeepNote {
        start_ms: 12808,
        duration_ms: 90,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 12898,
        duration_ms: 20,
        frequency: 247,
    },
    BeepNote {
        start_ms: 12898,
        duration_ms: 180,
        frequency: 1245,
    },
    BeepNote {
        start_ms: 13078,
        duration_ms: 92,
        frequency: 185,
    },
    BeepNote {
        start_ms: 13170,
        duration_ms: 93,
        frequency: 880,
    },
    BeepNote {
        start_ms: 13263,
        duration_ms: 20,
        frequency: 185,
    },
    BeepNote {
        start_ms: 13263,
        duration_ms: 81,
        frequency: 988,
    },
    BeepNote {
        start_ms: 13343,
        duration_ms: 92,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 13436,
        duration_ms: 20,
        frequency: 185,
    },
    BeepNote {
        start_ms: 13436,
        duration_ms: 90,
        frequency: 880,
    },
    BeepNote {
        start_ms: 13526,
        duration_ms: 90,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 13616,
        duration_ms: 20,
        frequency: 185,
    },
    BeepNote {
        start_ms: 13616,
        duration_ms: 90,
        frequency: 988,
    },
    BeepNote {
        start_ms: 13706,
        duration_ms: 90,
        frequency: 880,
    },
    BeepNote {
        start_ms: 13796,
        duration_ms: 90,
        frequency: 147,
    },
    BeepNote {
        start_ms: 13886,
        duration_ms: 90,
        frequency: 988,
    },
    BeepNote {
        start_ms: 13976,
        duration_ms: 20,
        frequency: 294,
    },
    BeepNote {
        start_ms: 13976,
        duration_ms: 90,
        frequency: 880,
    },
    BeepNote {
        start_ms: 14066,
        duration_ms: 90,
        frequency: 1175,
    },
    BeepNote {
        start_ms: 14156,
        duration_ms: 20,
        frequency: 147,
    },
    BeepNote {
        start_ms: 14156,
        duration_ms: 92,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 14248,
        duration_ms: 93,
        frequency: 1175,
    },
    BeepNote {
        start_ms: 14341,
        duration_ms: 20,
        frequency: 294,
    },
    BeepNote {
        start_ms: 14341,
        duration_ms: 92,
        frequency: 880,
    },
    BeepNote {
        start_ms: 14433,
        duration_ms: 81,
        frequency: 740,
    },
    BeepNote {
        start_ms: 14514,
        duration_ms: 20,
        frequency: 165,
    },
    BeepNote {
        start_ms: 14514,
        duration_ms: 180,
        frequency: 988,
    },
    BeepNote {
        start_ms: 14695,
        duration_ms: 124,
        frequency: 330,
    },
    BeepNote {
        start_ms: 14819,
        duration_ms: 56,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 14875,
        duration_ms: 20,
        frequency: 165,
    },
    BeepNote {
        start_ms: 14875,
        duration_ms: 124,
        frequency: 1661,
    },
    BeepNote {
        start_ms: 14999,
        duration_ms: 90,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 15089,
        duration_ms: 90,
        frequency: 330,
    },
    BeepNote {
        start_ms: 15179,
        duration_ms: 90,
        frequency: 831,
    },
    BeepNote {
        start_ms: 15269,
        duration_ms: 20,
        frequency: 220,
    },
    BeepNote {
        start_ms: 15269,
        duration_ms: 92,
        frequency: 988,
    },
    BeepNote {
        start_ms: 15362,
        duration_ms: 93,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 15454,
        duration_ms: 81,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 15535,
        duration_ms: 92,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 15628,
        duration_ms: 20,
        frequency: 208,
    },
    BeepNote {
        start_ms: 15628,
        duration_ms: 90,
        frequency: 831,
    },
    BeepNote {
        start_ms: 15718,
        duration_ms: 90,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 15808,
        duration_ms: 20,
        frequency: 220,
    },
    BeepNote {
        start_ms: 15808,
        duration_ms: 90,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 15897,
        duration_ms: 90,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 15987,
        duration_ms: 90,
        frequency: 185,
    },
    BeepNote {
        start_ms: 16077,
        duration_ms: 90,
        frequency: 880,
    },
    BeepNote {
        start_ms: 16168,
        duration_ms: 20,
        frequency: 370,
    },
    BeepNote {
        start_ms: 16168,
        duration_ms: 90,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 16258,
        duration_ms: 90,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 16348,
        duration_ms: 20,
        frequency: 185,
    },
    BeepNote {
        start_ms: 16348,
        duration_ms: 92,
        frequency: 880,
    },
    BeepNote {
        start_ms: 16440,
        duration_ms: 93,
        frequency: 2217,
    },
    BeepNote {
        start_ms: 16532,
        duration_ms: 20,
        frequency: 370,
    },
    BeepNote {
        start_ms: 16532,
        duration_ms: 81,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 16613,
        duration_ms: 92,
        frequency: 1175,
    },
    BeepNote {
        start_ms: 16706,
        duration_ms: 20,
        frequency: 147,
    },
    BeepNote {
        start_ms: 16706,
        duration_ms: 180,
        frequency: 988,
    },
    BeepNote {
        start_ms: 16886,
        duration_ms: 20,
        frequency: 294,
    },
    BeepNote {
        start_ms: 16886,
        duration_ms: 90,
        frequency: 880,
    },
    BeepNote {
        start_ms: 16976,
        duration_ms: 90,
        frequency: 1175,
    },
    BeepNote {
        start_ms: 17066,
        duration_ms: 20,
        frequency: 147,
    },
    BeepNote {
        start_ms: 17066,
        duration_ms: 92,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 17158,
        duration_ms: 92,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 17251,
        duration_ms: 20,
        frequency: 294,
    },
    BeepNote {
        start_ms: 17251,
        duration_ms: 93,
        frequency: 1175,
    },
    BeepNote {
        start_ms: 17343,
        duration_ms: 81,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 17424,
        duration_ms: 20,
        frequency: 165,
    },
    BeepNote {
        start_ms: 17424,
        duration_ms: 91,
        frequency: 988,
    },
    BeepNote {
        start_ms: 17515,
        duration_ms: 90,
        frequency: 659,
    },
    BeepNote {
        start_ms: 17605,
        duration_ms: 20,
        frequency: 330,
    },
    BeepNote {
        start_ms: 17605,
        duration_ms: 90,
        frequency: 988,
    },
    BeepNote {
        start_ms: 17695,
        duration_ms: 90,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 17785,
        duration_ms: 20,
        frequency: 165,
    },
    BeepNote {
        start_ms: 17785,
        duration_ms: 124,
        frequency: 1661,
    },
    BeepNote {
        start_ms: 17909,
        duration_ms: 90,
        frequency: 1976,
    },
    BeepNote {
        start_ms: 17999,
        duration_ms: 20,
        frequency: 330,
    },
    BeepNote {
        start_ms: 17999,
        duration_ms: 90,
        frequency: 2637,
    },
    BeepNote {
        start_ms: 18089,
        duration_ms: 90,
        frequency: 1661,
    },
    BeepNote {
        start_ms: 18179,
        duration_ms: 20,
        frequency: 220,
    },
    BeepNote {
        start_ms: 18179,
        duration_ms: 90,
        frequency: 880,
    },
    BeepNote {
        start_ms: 18269,
        duration_ms: 90,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 18359,
        duration_ms: 20,
        frequency: 220,
    },
    BeepNote {
        start_ms: 18359,
        duration_ms: 90,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 18449,
        duration_ms: 90,
        frequency: 2217,
    },
    BeepNote {
        start_ms: 18539,
        duration_ms: 20,
        frequency: 220,
    },
    BeepNote {
        start_ms: 18539,
        duration_ms: 93,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 18632,
        duration_ms: 92,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 18724,
        duration_ms: 20,
        frequency: 220,
    },
    BeepNote {
        start_ms: 18724,
        duration_ms: 173,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 18897,
        duration_ms: 90,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 18987,
        duration_ms: 90,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 19077,
        duration_ms: 90,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 19168,
        duration_ms: 90,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 19258,
        duration_ms: 20,
        frequency: 185,
    },
    BeepNote {
        start_ms: 19258,
        duration_ms: 92,
        frequency: 880,
    },
    BeepNote {
        start_ms: 19350,
        duration_ms: 93,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 19442,
        duration_ms: 20,
        frequency: 370,
    },
    BeepNote {
        start_ms: 19442,
        duration_ms: 81,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 19523,
        duration_ms: 92,
        frequency: 1109,
    },
    BeepNote {
        start_ms: 19616,
        duration_ms: 20,
        frequency: 147,
    },
    BeepNote {
        start_ms: 19616,
        duration_ms: 90,
        frequency: 988,
    },
    BeepNote {
        start_ms: 19706,
        duration_ms: 90,
        frequency: 1175,
    },
    BeepNote {
        start_ms: 19796,
        duration_ms: 90,
        frequency: 294,
    },
    BeepNote {
        start_ms: 19886,
        duration_ms: 90,
        frequency: 1175,
    },
    BeepNote {
        start_ms: 19976,
        duration_ms: 20,
        frequency: 147,
    },
    BeepNote {
        start_ms: 19976,
        duration_ms: 90,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 20066,
        duration_ms: 90,
        frequency: 1175,
    },
    BeepNote {
        start_ms: 20156,
        duration_ms: 20,
        frequency: 294,
    },
    BeepNote {
        start_ms: 20156,
        duration_ms: 90,
        frequency: 880,
    },
    BeepNote {
        start_ms: 20246,
        duration_ms: 90,
        frequency: 1175,
    },
    BeepNote {
        start_ms: 20336,
        duration_ms: 92,
        frequency: 165,
    },
    BeepNote {
        start_ms: 20428,
        duration_ms: 92,
        frequency: 988,
    },
    BeepNote {
        start_ms: 20521,
        duration_ms: 93,
        frequency: 330,
    },
    BeepNote {
        start_ms: 20613,
        duration_ms: 81,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 20694,
        duration_ms: 91,
        frequency: 165,
    },
    BeepNote {
        start_ms: 20785,
        duration_ms: 90,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 20875,
        duration_ms: 20,
        frequency: 330,
    },
    BeepNote {
        start_ms: 20875,
        duration_ms: 90,
        frequency: 1661,
    },
    BeepNote {
        start_ms: 20965,
        duration_ms: 90,
        frequency: 1976,
    },
    BeepNote {
        start_ms: 21055,
        duration_ms: 20,
        frequency: 220,
    },
    BeepNote {
        start_ms: 21055,
        duration_ms: 127,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 21182,
        duration_ms: 58,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 21239,
        duration_ms: 20,
        frequency: 220,
    },
    BeepNote {
        start_ms: 21239,
        duration_ms: 93,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 21332,
        duration_ms: 81,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 21413,
        duration_ms: 20,
        frequency: 208,
    },
    BeepNote {
        start_ms: 21413,
        duration_ms: 125,
        frequency: 1976,
    },
    BeepNote {
        start_ms: 21537,
        duration_ms: 56,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 21594,
        duration_ms: 124,
        frequency: 220,
    },
    BeepNote {
        start_ms: 21717,
        duration_ms: 56,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 21774,
        duration_ms: 20,
        frequency: 185,
    },
    BeepNote {
        start_ms: 21774,
        duration_ms: 180,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 21987,
        duration_ms: 90,
        frequency: 370,
    },
    BeepNote {
        start_ms: 22077,
        duration_ms: 90,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 22168,
        duration_ms: 20,
        frequency: 185,
    },
    BeepNote {
        start_ms: 22168,
        duration_ms: 90,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 22258,
        duration_ms: 90,
        frequency: 2217,
    },
    BeepNote {
        start_ms: 22348,
        duration_ms: 90,
        frequency: 370,
    },
    BeepNote {
        start_ms: 22438,
        duration_ms: 90,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 22528,
        duration_ms: 20,
        frequency: 147,
    },
    BeepNote {
        start_ms: 22528,
        duration_ms: 180,
        frequency: 988,
    },
    BeepNote {
        start_ms: 22712,
        duration_ms: 20,
        frequency: 294,
    },
    BeepNote {
        start_ms: 22712,
        duration_ms: 173,
        frequency: 1175,
    },
    BeepNote {
        start_ms: 22886,
        duration_ms: 90,
        frequency: 147,
    },
    BeepNote {
        start_ms: 22976,
        duration_ms: 90,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 23066,
        duration_ms: 20,
        frequency: 294,
    },
    BeepNote {
        start_ms: 23066,
        duration_ms: 90,
        frequency: 1480,
    },
    BeepNote {
        start_ms: 23156,
        duration_ms: 90,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 23246,
        duration_ms: 20,
        frequency: 165,
    },
    BeepNote {
        start_ms: 23246,
        duration_ms: 92,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 23338,
        duration_ms: 92,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 23431,
        duration_ms: 93,
        frequency: 165,
    },
    BeepNote {
        start_ms: 23523,
        duration_ms: 81,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 23604,
        duration_ms: 91,
        frequency: 165,
    },
    BeepNote {
        start_ms: 23695,
        duration_ms: 90,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 23785,
        duration_ms: 90,
        frequency: 165,
    },
    BeepNote {
        start_ms: 23875,
        duration_ms: 90,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 23965,
        duration_ms: 124,
        frequency: 220,
    },
    BeepNote {
        start_ms: 24089,
        duration_ms: 56,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 24145,
        duration_ms: 20,
        frequency: 220,
    },
    BeepNote {
        start_ms: 24145,
        duration_ms: 124,
        frequency: 1319,
    },
    BeepNote {
        start_ms: 24269,
        duration_ms: 56,
        frequency: 2217,
    },
    BeepNote {
        start_ms: 24325,
        duration_ms: 20,
        frequency: 220,
    },
    BeepNote {
        start_ms: 24325,
        duration_ms: 180,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 24509,
        duration_ms: 20,
        frequency: 220,
    },
    BeepNote {
        start_ms: 24509,
        duration_ms: 116,
        frequency: 1760,
    },
    BeepNote {
        start_ms: 24625,
        duration_ms: 90,
        frequency: 1480,
    },
];
